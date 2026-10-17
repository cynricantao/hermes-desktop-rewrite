//! Admin extras: cron (blueprints, delivery targets, job read/update, `cron.manage`), profile
//! extras (active profile, description, model, describe-auto, export/import, setup command,
//! open-terminal, desktop overlay, roster/editor RPCs, avatar assets) and memory providers
//! (config schema, setup, OAuth).
//!
//! Follows the conventions in [`crate::api`]. Profile create/rename/delete/soul and the cron
//! create/pause/resume/trigger/delete routes are already covered by [`crate::capabilities`];
//! this module does not duplicate them.

use serde_json::{Value, json};

use crate::api::{
    ApiError, Call, Method, Request, call_as, call_of, explicit_profile_query, profile_query, req, seg, send_as,
    with_query,
};
use crate::http::Rest;
use crate::rpc::Client;

/// Add a named profile to RPC params only when the caller named one (`""` = the gateway's own).
fn with_profile(mut params: Value, profile: &str) -> Value {
    if !profile.is_empty() {
        params["profile"] = json!(profile);
    }
    params
}

// ── Cron blueprints ─────────────────────────────────────────────────────────────

/// One slot of a blueprint's form schema. The wire key `type` is `field_type` here.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct CronBlueprintField {
    pub name: String,
    #[serde(rename = "type")]
    pub field_type: String,
    pub label: String,
    /// Prefill value (string, number, bool or null, depending on the slot type).
    pub default: Value,
    /// Allowed values for `select`/`multiselect` slots.
    pub options: Vec<String>,
    pub optional: bool,
    pub strict: bool,
    pub help: Option<String>,
}

/// One blueprint catalog entry: a form schema plus a ready-to-paste command and app deep link.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct CronBlueprint {
    pub key: String,
    pub title: String,
    pub description: String,
    pub category: String,
    pub tags: Vec<String>,
    pub fields: Vec<CronBlueprintField>,
    /// `"builtin"` or `"plugin"`.
    pub source: String,
    pub plugin: Option<String>,
    /// Schedule template; `{hour}`/`{minute}`-style placeholders are filled from the form.
    pub schedule: String,
    /// Human-readable schedule ("Every day at 08:00"), filled in best-effort.
    #[serde(rename = "scheduleHuman")]
    pub schedule_human: String,
    /// Ready-to-paste `/blueprint <key> slot=value …` command.
    pub command: String,
    /// `hermes://` deep link for the desktop app.
    #[serde(rename = "appUrl")]
    pub app_url: String,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct CronBlueprints {
    pub blueprints: Vec<CronBlueprint>,
}

/// `GET /api/cron/blueprints`: built-in + plugin blueprint form schemas for `profile`.
pub fn cron_blueprints_request(profile: &str) -> Request {
    req(
        Method::Get,
        format!("/api/cron/blueprints{}", profile_query(profile)),
        None,
    )
}

pub async fn cron_blueprints(rest: &Rest, profile: &str) -> Result<CronBlueprints, ApiError> {
    send_as(rest, &cron_blueprints_request(profile)).await
}

/// `POST /api/cron/blueprints/instantiate`: fill a blueprint's slots and create its job.
/// `values` is an object of slot name → value.
///
/// **Creates a scheduled job** (it starts firing on the resulting schedule).
pub fn cron_blueprint_instantiate_request(blueprint: &str, values: &Value, profile: &str) -> Request {
    req(
        Method::Post,
        format!("/api/cron/blueprints/instantiate{}", explicit_profile_query(profile)),
        Some(json!({"blueprint": blueprint, "values": values})),
    )
}

pub async fn cron_blueprint_instantiate(
    rest: &Rest,
    blueprint: &str,
    values: &Value,
    profile: &str,
) -> Result<CronJob, ApiError> {
    send_as(rest, &cron_blueprint_instantiate_request(blueprint, values, profile)).await
}

// ── Cron delivery targets ───────────────────────────────────────────────────────

/// One destination the cron form can deliver to (`local` is always first).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct CronDeliveryTarget {
    pub id: String,
    pub name: String,
    /// False when the platform is configured but has no cron home channel yet.
    pub home_target_set: bool,
    /// Env var the home channel comes from, when it does.
    pub home_env_var: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct CronDeliveryTargets {
    pub targets: Vec<CronDeliveryTarget>,
}

/// `GET /api/cron/delivery-targets`: implicit `local` plus the profile's gateway platforms.
pub fn cron_delivery_targets_request(profile: &str) -> Request {
    req(
        Method::Get,
        format!("/api/cron/delivery-targets{}", profile_query(profile)),
        None,
    )
}

pub async fn cron_delivery_targets(rest: &Rest, profile: &str) -> Result<CronDeliveryTargets, ApiError> {
    send_as(rest, &cron_delivery_targets_request(profile)).await
}

// ── Cron jobs (read / update) ───────────────────────────────────────────────────

/// A cron job record as the dashboard returns it (`cron/jobs.py::_normalize_job_record`):
/// legacy fields are filled in, so `id`, `name`, `prompt` and `schedule_display` are always
/// strings.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct CronJob {
    pub id: String,
    pub name: String,
    pub prompt: String,
    /// String schedule (`"0 9 * * *"`, `"every 30m"`, ISO one-shot) or a structured object.
    pub schedule: Value,
    pub schedule_display: String,
    /// Derived state: `"active"`, `"paused"`, `"completed"`, …
    pub state: String,
    pub enabled: bool,
    pub paused_at: Value,
    pub next_run_at: Value,
    pub last_run_at: Value,
    pub last_status: Option<String>,
    pub deliver: Option<String>,
    pub repeat: Value,
    pub skills: Vec<String>,
    pub skill: Option<String>,
    pub script: Option<String>,
    /// Most recent run record, when the store keeps one.
    pub latest_execution: Value,
}

/// `GET /api/cron/jobs/{id}`: one job. `profile` restricts the search; `""` searches every
/// profile (the first match wins).
pub fn cron_job_request(id: &str, profile: &str) -> Request {
    req(
        Method::Get,
        format!("/api/cron/jobs/{}{}", seg(id), profile_query(profile)),
        None,
    )
}

pub async fn cron_job(rest: &Rest, id: &str, profile: &str) -> Result<CronJob, ApiError> {
    send_as(rest, &cron_job_request(id, profile)).await
}

/// `PUT /api/cron/jobs/{id}`: patch a job. `updates` is the partial record to merge — any of
/// `prompt`, `schedule`, `name`, `deliver`, `repeat`, `skills`, `script`, …; pass only the
/// fields to change.
pub fn cron_job_update_request(id: &str, updates: &Value, profile: &str) -> Request {
    req(
        Method::Put,
        format!("/api/cron/jobs/{}{}", seg(id), profile_query(profile)),
        Some(json!({"updates": updates})),
    )
}

/// **Changes a schedule** (the returned record is the job after the merge).
pub async fn cron_job_update(rest: &Rest, id: &str, updates: &Value, profile: &str) -> Result<CronJob, ApiError> {
    send_as(rest, &cron_job_update_request(id, updates, profile)).await
}

// ── cron.manage (JSON-RPC) ─────────────────────────────────────────────────────

/// One job as the `cron.manage` tool renders it (`tools/cronjob_job_args.py::_format_job`):
/// the id lives under `job_id`, the prompt is a `prompt_preview`, and `schedule` is already
/// the display form. The minimal `removed_job` row uses `id` instead of `job_id`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct CronManageJob {
    pub job_id: String,
    /// Set on the minimal `removed_job` row instead of `job_id`.
    pub id: String,
    pub name: String,
    pub skill: Option<String>,
    pub skills: Vec<String>,
    pub prompt_preview: String,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub pinned: bool,
    /// Display form of the schedule.
    pub schedule: String,
    pub repeat: Value,
    pub deliver: Option<String>,
    pub next_run_at: Value,
    pub last_run_at: Value,
    pub last_status: Option<String>,
    pub enabled: bool,
    pub state: String,
    pub paused_at: Value,
    pub paused_reason: Option<String>,
}

impl CronManageJob {
    /// The job id, whichever key the row carried.
    pub fn id(&self) -> &str {
        if self.job_id.is_empty() { &self.id } else { &self.job_id }
    }
}

/// Result envelope of `cron.manage`. The cron tool reports failures as a tool-level `error`
/// inside a successful RPC result, so callers must inspect this struct, not only the `Result`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct CronManageResult {
    pub success: Option<bool>,
    /// Tool-level failure message (the RPC itself succeeded).
    pub error: Option<String>,
    pub message: Option<String>,
    pub count: Option<u64>,
    pub jobs: Vec<CronManageJob>,
    /// Created/paused/resumed job.
    pub job: Option<CronManageJob>,
    /// Minimal `{id, name, schedule}` of a removed job.
    pub removed_job: Option<CronManageJob>,
    /// Profile the gateway proved it honoured, echoed when one was named.
    pub scoped: Option<String>,
}

/// `cron.manage` `action:"list"`: the profile's jobs (paused ones only with
/// `include_disabled`).
pub fn cron_manage_list_call(include_disabled: bool, profile: &str) -> Call {
    let params = with_profile(json!({"action": "list", "include_disabled": include_disabled}), profile);
    call_of("cron.manage", params)
}

pub async fn cron_manage_list(
    client: &Client,
    include_disabled: bool,
    profile: &str,
) -> Result<CronManageResult, ApiError> {
    call_as(client, &cron_manage_list_call(include_disabled, profile)).await
}

/// `cron.manage` `action:"add"`: create a job. `name` is the job id/name reference, `deliver`
/// a target id (`"local"`, `"telegram"`, `"bot-chat[:name]"`; empty = the tool default),
/// `repeat` a run count and `continuity` whether runs share one session.
///
/// **Creates a scheduled job.**
pub fn cron_manage_add_call(
    name: &str,
    schedule: &str,
    prompt: &str,
    deliver: &str,
    repeat: Option<u32>,
    continuity: Option<bool>,
    profile: &str,
) -> Call {
    let mut params = json!({"action": "add", "name": name, "schedule": schedule, "prompt": prompt});
    if !deliver.is_empty() {
        params["deliver"] = json!(deliver);
    }
    if let Some(repeat) = repeat {
        params["repeat"] = json!(repeat);
    }
    if let Some(continuity) = continuity {
        params["continuity"] = json!(continuity);
    }
    call_of("cron.manage", with_profile(params, profile))
}

// Mirrors the RPC's parameter surface; splitting it would only hide the mapping.
#[allow(clippy::too_many_arguments)]
pub async fn cron_manage_add(
    client: &Client,
    name: &str,
    schedule: &str,
    prompt: &str,
    deliver: &str,
    repeat: Option<u32>,
    continuity: Option<bool>,
    profile: &str,
) -> Result<CronManageResult, ApiError> {
    call_as(
        client,
        &cron_manage_add_call(name, schedule, prompt, deliver, repeat, continuity, profile),
    )
    .await
}

/// `cron.manage` `action:"remove"|"pause"|"resume"` for one job id/name.
///
/// **Destructive** for `remove`; `pause`/`resume` change when it runs.
pub fn cron_manage_job_call(action: &str, job: &str, profile: &str) -> Call {
    let params = with_profile(json!({"action": action, "name": job}), profile);
    call_of("cron.manage", params)
}

pub async fn cron_manage_job(
    client: &Client,
    action: &str,
    job: &str,
    profile: &str,
) -> Result<CronManageResult, ApiError> {
    call_as(client, &cron_manage_job_call(action, job, profile)).await
}

// ── Profile: active, description, model ────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ProfileActiveSet {
    pub ok: bool,
    /// The normalized sticky default that new CLI commands will pick up.
    pub active: String,
}

/// `POST /api/profiles/active`: set the sticky default profile (`hermes profile use`).
///
/// **Changes the CLI/gateway default** for future commands; the running dashboard keeps its
/// own profile.
pub fn profile_active_set_request(name: &str) -> Request {
    req(Method::Post, "/api/profiles/active", Some(json!({"name": name})))
}

pub async fn profile_active_set(rest: &Rest, name: &str) -> Result<ProfileActiveSet, ApiError> {
    send_as(rest, &profile_active_set_request(name)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ProfileDescriptionSet {
    pub ok: bool,
    pub description: String,
    /// Always false after this call: the text is user-authored, so auto-describe won't
    /// overwrite it.
    pub description_auto: bool,
}

/// `PUT /api/profiles/{name}/description`: set (or clear, with `""`) a profile's role
/// description and mark it user-authored.
pub fn profile_description_set_request(name: &str, description: &str) -> Request {
    req(
        Method::Put,
        format!("/api/profiles/{}/description", seg(name)),
        Some(json!({"description": description})),
    )
}

pub async fn profile_description_set(
    rest: &Rest,
    name: &str,
    description: &str,
) -> Result<ProfileDescriptionSet, ApiError> {
    send_as(rest, &profile_description_set_request(name, description)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ProfileModelSet {
    pub ok: bool,
    pub provider: String,
    pub model: String,
}

/// `PUT /api/profiles/{name}/model`: set the profile's main model. The backend validates the
/// pick against its provider catalog and answers 400 for an unknown provider/model.
pub fn profile_model_set_request(name: &str, provider: &str, model: &str) -> Request {
    req(
        Method::Put,
        format!("/api/profiles/{}/model", seg(name)),
        Some(json!({"provider": provider, "model": model})),
    )
}

pub async fn profile_model_set(
    rest: &Rest,
    name: &str,
    provider: &str,
    model: &str,
) -> Result<ProfileModelSet, ApiError> {
    send_as(rest, &profile_model_set_request(name, provider, model)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ProfileDescribed {
    pub ok: bool,
    /// Why generation failed (HTTP 200 with `ok: false`); null on success.
    pub reason: Option<String>,
    pub description: String,
    pub description_auto: bool,
}

/// `POST /api/profiles/{name}/describe-auto`: generate a role description from the profile's
/// contents (`overwrite` replaces a user-authored one).
///
/// **Spends model calls** (one auxiliary-LLM round trip, up to 60 s).
pub fn profile_describe_auto_request(name: &str, overwrite: bool) -> Request {
    req(
        Method::Post,
        format!("/api/profiles/{}/describe-auto", seg(name)),
        Some(json!({"overwrite": overwrite})),
    )
}

pub async fn profile_describe_auto(rest: &Rest, name: &str, overwrite: bool) -> Result<ProfileDescribed, ApiError> {
    send_as(rest, &profile_describe_auto_request(name, overwrite)).await
}

// ── Profile: export / import ───────────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ProfileExported {
    pub ok: bool,
    /// Path of the written `.tar.gz` on the backend's filesystem.
    pub archive: String,
}

/// `POST /api/profiles/{name}/export`: bundle the profile into a `.tar.gz` at `output`
/// (empty = the backend's export directory). `extra_files` maps a root filename → text to
/// bundle alongside the profile.
///
/// **Writes an archive** on the backend host.
pub fn profile_export_request(name: &str, output: &str, extra_files: &Value) -> Request {
    req(
        Method::Post,
        format!("/api/profiles/{}/export", seg(name)),
        Some(json!({"output": output, "extra_files": extra_files})),
    )
}

pub async fn profile_export(
    rest: &Rest,
    name: &str,
    output: &str,
    extra_files: &Value,
) -> Result<ProfileExported, ApiError> {
    send_as(rest, &profile_export_request(name, output, extra_files)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ProfileImported {
    pub ok: bool,
    /// The profile id the archive landed under.
    pub name: String,
    pub path: String,
    /// Bundled `desktop.json` appearance overlay, when the archive carried one.
    pub desktop: Option<Value>,
}

/// `POST /api/profiles/import`: import a profile archive from the backend's filesystem;
/// `name` overrides the name inferred from the archive root (empty = infer).
///
/// **Creates a profile** and writes its files.
pub fn profile_import_request(archive: &str, name: &str) -> Request {
    let mut body = json!({"archive": archive});
    if !name.is_empty() {
        body["name"] = json!(name);
    }
    req(Method::Post, "/api/profiles/import", Some(body))
}

pub async fn profile_import(rest: &Rest, archive: &str, name: &str) -> Result<ProfileImported, ApiError> {
    send_as(rest, &profile_import_request(archive, name)).await
}

// ── Profile: setup command, terminal, desktop overlay ──────────────────────────

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ProfileSetupCommand {
    pub command: String,
}

/// `GET /api/profiles/{name}/setup-command`: the shell command that sets up and launches the
/// profile.
pub fn profile_setup_command_request(name: &str) -> Request {
    req(Method::Get, format!("/api/profiles/{}/setup-command", seg(name)), None)
}

pub async fn profile_setup_command(rest: &Rest, name: &str) -> Result<ProfileSetupCommand, ApiError> {
    send_as(rest, &profile_setup_command_request(name)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ProfileTerminalOpened {
    pub ok: bool,
    /// The command the terminal was opened with.
    pub command: String,
}

/// `POST /api/profiles/{name}/open-terminal`: open the host's terminal emulator running the
/// profile's setup command.
///
/// **Spawns a process** on the backend host (needs a desktop session; fails headless).
pub fn profile_open_terminal_request(name: &str) -> Request {
    req(Method::Post, format!("/api/profiles/{}/open-terminal", seg(name)), None)
}

pub async fn profile_open_terminal(rest: &Rest, name: &str) -> Result<ProfileTerminalOpened, ApiError> {
    send_as(rest, &profile_open_terminal_request(name)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ProfileDesktopOverlay {
    pub exists: bool,
    /// Parsed `desktop.json` (appearance/interface overlay); null when `exists` is false.
    pub desktop: Option<Value>,
}

/// `GET /api/profiles/{name}/desktop-overlay`: the appearance overlay bundled with an imported
/// profile.
pub fn profile_desktop_overlay_request(name: &str) -> Request {
    req(
        Method::Get,
        format!("/api/profiles/{}/desktop-overlay", seg(name)),
        None,
    )
}

pub async fn profile_desktop_overlay(rest: &Rest, name: &str) -> Result<ProfileDesktopOverlay, ApiError> {
    send_as(rest, &profile_desktop_overlay_request(name)).await
}

// ── Profile RPCs: roster, editor snapshot, configure, assets ───────────────────

/// One roster row from `profiles.list`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ProfileRow {
    pub name: String,
    pub path: String,
    pub is_default: bool,
    /// Main model id; null before one is pinned.
    pub model: Option<String>,
    /// Provider of the main model; null when none is pinned.
    pub provider: Option<String>,
    pub description: String,
    pub display_name: String,
    pub skill_count: u64,
    pub previous_names: Vec<String>,
    /// Presentation metadata (accent, icon, …), when the profile has any.
    pub ui_meta: Option<Value>,
    /// Revision map for optimistic-concurrency writes of `ui_meta`.
    pub ui_meta_revisions: Value,
    pub has_avatar: Option<bool>,
    /// Newest human-facing session row, when `include_sessions` was on.
    pub last_session: Option<Value>,
    /// Newest denied (kanban/tool worker) session row — proves the profile is working.
    pub worker_session: Option<Value>,
    pub canonical_session: Option<Value>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ProfileList {
    pub profiles: Vec<ProfileRow>,
    pub bot_mode_protocol: bool,
    /// Machine id answering the list (multi-connection clients can prove which backend did).
    pub install_id: String,
}

/// `profiles.list` (JSON-RPC twin of `GET /api/profiles`). `include_sessions` adds the
/// last/worker/canonical session rows so a roster paints previews without extra calls.
pub fn profiles_list_call(include_sessions: bool) -> Call {
    call_of("profiles.list", json!({"include_sessions": include_sessions}))
}

pub async fn profiles_list(client: &Client, include_sessions: bool) -> Result<ProfileList, ApiError> {
    call_as(client, &profiles_list_call(include_sessions)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ProfileModel {
    pub provider: String,
    /// The profile's default model id (wire key `default`).
    pub default: String,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ProfileSkill {
    pub name: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ProfileToolset {
    pub name: String,
    pub label: String,
    pub description: String,
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ProfileMcpServer {
    pub name: String,
    pub enabled: bool,
    /// `"stdio"` or `"http"`.
    pub transport: String,
}

/// Editor snapshot from `profiles.describe`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ProfileSnapshot {
    pub name: String,
    pub description: String,
    pub soul: String,
    pub model: ProfileModel,
    pub skills: Vec<ProfileSkill>,
    pub toolsets: Vec<ProfileToolset>,
    /// Whether `toolsets` reflects an explicit pin (rather than the platform default).
    pub toolsets_pinned: bool,
    pub mcp_servers: Vec<ProfileMcpServer>,
}

/// `profiles.describe`: the editor's read of one profile (soul, model, skills, toolsets, MCP).
pub fn profile_describe_call(name: &str) -> Call {
    call_of("profiles.describe", json!({"name": name}))
}

pub async fn profile_describe(client: &Client, name: &str) -> Result<ProfileSnapshot, ApiError> {
    call_as(client, &profile_describe_call(name)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ProfileConfigured {
    pub ok: bool,
    /// Per-section success flags (`soul`, `description`, `model`, `ui_meta`, …).
    pub applied: Value,
    /// Set when the pick needs an explicit confirmation (an expensive model); re-send the call
    /// with `confirm_expensive_model: true` in `patch`.
    pub confirm_required: Option<bool>,
    pub confirm_message: Option<String>,
}

/// `profiles.configure`: the editor's Save. `patch` is an object holding any of `ui_meta`
/// (+ `ui_meta_expected_revisions`), `soul`, `description`, `model` + `provider`
/// (and `confirm_expensive_model`), `disabled_skills`, `enabled_toolsets`,
/// `enabled_mcp_servers`; sections are independent and `applied` reports each.
///
/// **May switch the profile's model**; a gated model echoes `confirm_required`.
pub fn profile_configure_call(name: &str, patch: &Value) -> Call {
    let mut params = match patch {
        Value::Object(map) => Value::Object(map.clone()),
        _ => json!({}),
    };
    params["name"] = json!(name);
    call_of("profiles.configure", params)
}

pub async fn profile_configure(client: &Client, name: &str, patch: &Value) -> Result<ProfileConfigured, ApiError> {
    call_as(client, &profile_configure_call(name, patch)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ProfileAssetSet {
    pub ok: bool,
    pub asset: String,
    /// Stored bytes (0 when clearing).
    pub size: Option<u64>,
    /// Files removed by a `clear` call.
    pub removed: Option<u64>,
}

/// `profiles.set_asset`: store a profile asset (`asset` is `"avatar"`), from a data URL or
/// bare base64 (PNG/JPEG/WebP, ≤ 2 MB; the backend sniffs the real format). `clear` removes
/// it instead of writing.
///
/// **Writes a file** in the profile.
pub fn profiles_set_asset_call(name: &str, asset: &str, data: &str, clear: bool) -> Call {
    let mut params = json!({"name": name, "asset": asset});
    if clear {
        params["clear"] = json!(true);
    } else {
        params["data"] = json!(data);
    }
    call_of("profiles.set_asset", params)
}

pub async fn profiles_set_asset(
    client: &Client,
    name: &str,
    asset: &str,
    data: &str,
    clear: bool,
) -> Result<ProfileAssetSet, ApiError> {
    call_as(client, &profiles_set_asset_call(name, asset, data, clear)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ProfileAssetData {
    pub found: bool,
    pub mime: String,
    pub size: u64,
    /// `data:<mime>;base64,…` when found.
    pub data: String,
}

/// `profiles.get_asset`: the asset as a data URL; `found: false` when the profile has none
/// (not an error).
pub fn profiles_get_asset_call(name: &str, asset: &str) -> Call {
    call_of("profiles.get_asset", json!({"name": name, "asset": asset}))
}

pub async fn profiles_get_asset(client: &Client, name: &str, asset: &str) -> Result<ProfileAssetData, ApiError> {
    call_as(client, &profiles_get_asset_call(name, asset)).await
}

/// Options for `profiles.create`: only the fields the New Profile dialog sends.
#[derive(Debug, Clone, Default)]
pub struct ProfileCreateOptions {
    pub description: String,
    /// Source profile to clone; empty = a fresh profile with the bundled skills.
    pub clone_from: String,
    /// Clone the source's config too (not just its soul/description).
    pub clone_all: bool,
    /// Keep the source's bot tokens/allowlists; off strips them so two profiles never share a bot.
    pub clone_channels: bool,
    /// Skip seeding the bundled skills.
    pub no_skills: bool,
    /// SOUL.md written at create time.
    pub soul: String,
    /// Model to pin (only when `provider` is set as well).
    pub model: String,
    pub provider: String,
    /// Share the launching profile's auth material.
    pub share_auth: bool,
    /// Skip the `hermes-<name>` wrapper script.
    pub no_alias: bool,
}

/// `profiles.create` reply (the RPC twin of `POST /api/profiles`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ProfileCreated {
    pub ok: bool,
    pub name: String,
    /// The profile directory the backend created.
    pub path: String,
    pub soul_written: bool,
    pub model_set: bool,
    /// Credential-mirroring report (`methods_profiles._mirror_launch_credentials`).
    pub mirrored: Value,
}

/// `profiles.create` (the RPC twin of the REST create already covered by
/// [`crate::capabilities`]). `mirror_credentials: Some(false)` stops the backend seeding
/// credentials from the launching profile; `None` keeps its default (`true`). Flags left at
/// their defaults are omitted, matching the REST body.
///
/// **Writes a profile directory** (and its alias script); builder + unit test only.
pub fn profiles_create_call(name: &str, options: &ProfileCreateOptions, mirror_credentials: Option<bool>) -> Call {
    let mut params = json!({ "name": name });
    for (key, value) in [
        ("description", options.description.clone()),
        ("clone_from", options.clone_from.clone()),
        ("soul", options.soul.clone()),
        ("model", options.model.clone()),
        ("provider", options.provider.clone()),
    ] {
        if !value.is_empty() {
            params[key] = json!(value);
        }
    }
    for (key, flag) in [
        ("clone_all", options.clone_all),
        ("clone_channels", options.clone_channels),
        ("no_skills", options.no_skills),
        ("share_auth", options.share_auth),
        ("no_alias", options.no_alias),
    ] {
        if flag {
            params[key] = json!(true);
        }
    }
    if let Some(v) = mirror_credentials {
        params["mirror_credentials"] = json!(v);
    }
    call_of("profiles.create", params)
}

/// Create a profile (RPC). **Writes a profile directory**; builder + unit test only — the REST
/// create in [`crate::capabilities`] stays the live-tested path.
pub async fn profiles_create(
    client: &Client,
    name: &str,
    options: &ProfileCreateOptions,
    mirror_credentials: Option<bool>,
) -> Result<ProfileCreated, ApiError> {
    call_as(client, &profiles_create_call(name, options, mirror_credentials)).await
}

// ── Memory providers ───────────────────────────────────────────────────────────

/// One option of a `select` field.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct MemoryFieldOption {
    pub value: String,
    pub label: String,
    pub description: Option<String>,
}

/// One config-form field of a memory provider.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct MemoryProviderField {
    pub key: String,
    pub label: String,
    /// `"text"`, `"secret"`, `"boolean"`, `"select"`, `"integer"`, `"number"`, …
    pub kind: String,
    pub description: Option<String>,
    pub info: Option<String>,
    pub placeholder: Option<String>,
    pub inline: Option<bool>,
    pub group: Option<String>,
    pub options: Vec<MemoryFieldOption>,
    /// Current value; secrets are write-only, so it is always `""` — read `is_set` instead.
    pub value: Value,
    pub is_set: Option<bool>,
    /// Legacy (non-declared) surface only.
    pub required: Option<bool>,
}

/// A memory provider's config form.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct MemoryProviderConfig {
    pub name: String,
    pub label: String,
    pub docs_url: String,
    pub fields: Vec<MemoryProviderField>,
    /// Legacy (non-declared) surface only: setup/install hints.
    pub setup: Option<Value>,
}

/// `GET /api/memory/providers/{name}/config`: the provider's config form. `surface` is
/// `"declared"` for the manifest-declared schema the desktop panel renders, `""` for the
/// legacy plugin surface. Secret fields come back write-only (`value: ""`, `is_set`).
pub fn memory_provider_config_request(name: &str, surface: &str, profile: &str) -> Request {
    let path = with_query(
        &format!("/api/memory/providers/{}/config", seg(name)),
        &[("surface", surface), ("profile", profile)],
    );
    req(Method::Get, path, None)
}

pub async fn memory_provider_config(
    rest: &Rest,
    name: &str,
    surface: &str,
    profile: &str,
) -> Result<MemoryProviderConfig, ApiError> {
    send_as(rest, &memory_provider_config_request(name, surface, profile)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct MemoryProviderConfigUpdated {
    pub ok: bool,
    /// The provider now marked active (declared surface only).
    pub active: Option<String>,
}

/// `PUT /api/memory/providers/{name}/config`: save form values. Secret fields are written to
/// the profile's `.env`; the declared surface also marks the provider active.
///
/// **Writes config and secrets.**
pub fn memory_provider_config_update_request(name: &str, values: &Value, surface: &str, profile: &str) -> Request {
    let path = with_query(
        &format!("/api/memory/providers/{}/config", seg(name)),
        &[("surface", surface), ("profile", profile)],
    );
    req(Method::Put, path, Some(json!({"values": values})))
}

pub async fn memory_provider_config_update(
    rest: &Rest,
    name: &str,
    values: &Value,
    surface: &str,
    profile: &str,
) -> Result<MemoryProviderConfigUpdated, ApiError> {
    send_as(
        rest,
        &memory_provider_config_update_request(name, values, surface, profile),
    )
    .await
}

/// One dependency/setup command's outcome.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct MemorySetupStep {
    /// `"pip"`, `"setup"`, `"external_check"`, `"external_install"`.
    pub kind: String,
    pub name: String,
    /// `"installed"`, `"already_installed"`, `"missing"`, `"failed"`, `"no_declared_steps"`, …
    pub status: String,
    pub command: String,
    pub returncode: Option<i64>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct MemorySetupResult {
    pub ok: bool,
    pub provider: String,
    pub results: Vec<MemorySetupStep>,
    /// The provider's discovery status row after setup (provider-defined shape).
    pub status: Option<Value>,
}

/// `POST /api/memory/providers/{name}/setup`: install the provider's Python/external
/// dependencies and run its declared setup commands. `values` (optional) persists form values
/// first.
///
/// **Spawns processes** (pip and shell commands declared by the plugin).
pub fn memory_provider_setup_request(name: &str, values: Option<&Value>, profile: &str) -> Request {
    let path = with_query(
        &format!("/api/memory/providers/{}/setup", seg(name)),
        &[("profile", profile)],
    );
    let body = match values {
        Some(values) => json!({"values": values}),
        None => json!({}),
    };
    req(Method::Post, path, Some(body))
}

pub async fn memory_provider_setup(
    rest: &Rest,
    name: &str,
    values: Option<&Value>,
    profile: &str,
) -> Result<MemorySetupResult, ApiError> {
    send_as(rest, &memory_provider_setup_request(name, values, profile)).await
}

/// `POST /api/memory/providers/{provider}/oauth/start`: begin the provider's zero-CLI OAuth
/// flow (browser + loopback listener). Returns immediately; poll
/// [`memory_provider_oauth_status`]. The payload is flow-defined, so it is returned raw.
///
/// **Network + opens a browser.**
pub fn memory_provider_oauth_start_request(provider: &str, profile: &str) -> Request {
    let path = with_query(
        &format!("/api/memory/providers/{}/oauth/start", seg(provider)),
        &[("profile", profile)],
    );
    req(Method::Post, path, None)
}

pub async fn memory_provider_oauth_start(rest: &Rest, provider: &str, profile: &str) -> Result<Value, ApiError> {
    crate::api::send(rest, &memory_provider_oauth_start_request(provider, profile)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct MemoryOAuthStatus {
    /// `"idle"`, `"pending"`, `"connected"` or `"error"`.
    pub status: String,
    pub error: Option<String>,
    pub detail: Option<String>,
}

/// `GET /api/memory/providers/{provider}/oauth/status`: poll the flow started by
/// [`memory_provider_oauth_start`].
pub fn memory_provider_oauth_status_request(provider: &str, profile: &str) -> Request {
    let path = with_query(
        &format!("/api/memory/providers/{}/oauth/status", seg(provider)),
        &[("profile", profile)],
    );
    req(Method::Get, path, None)
}

pub async fn memory_provider_oauth_status(
    rest: &Rest,
    provider: &str,
    profile: &str,
) -> Result<MemoryOAuthStatus, ApiError> {
    send_as(rest, &memory_provider_oauth_status_request(provider, profile)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::decode;

    #[test]
    fn blueprint_paths_and_decode() {
        assert_eq!(cron_blueprints_request("").path, "/api/cron/blueprints");
        assert_eq!(cron_blueprints_request("swe").path, "/api/cron/blueprints?profile=swe");

        let r = cron_blueprint_instantiate_request("morning-brief", &json!({"time": "08:00", "deliver": "local"}), "");
        assert_eq!(
            (r.method, r.path.as_str()),
            (Method::Post, "/api/cron/blueprints/instantiate?profile=default")
        );
        assert_eq!(
            r.body.unwrap(),
            json!({"blueprint": "morning-brief", "values": {"time": "08:00", "deliver": "local"}})
        );
        assert_eq!(
            cron_blueprint_instantiate_request("x", &json!({}), "swe").path,
            "/api/cron/blueprints/instantiate?profile=swe"
        );

        let fixture = json!({"blueprints": [{
            "key": "morning-brief", "title": "Morning brief", "description": "Overnight news",
            "category": "research", "tags": ["news", "daily"],
            "fields": [
                {"name": "time", "type": "time", "label": "Time", "default": "08:00",
                 "options": [], "optional": false, "strict": true, "help": "24h clock"},
                {"name": "deliver", "type": "select", "label": "Deliver to", "default": "local",
                 "options": ["origin", "local", "telegram"], "optional": true, "strict": false,
                 "help": null}
            ],
            "source": "builtin", "plugin": null, "schedule": "{minute} {hour} * * *",
            "scheduleHuman": "Every day at 08:00", "command": "/blueprint morning-brief time=08:00",
            "appUrl": "hermes://blueprint/morning-brief"
        }]});
        let b: CronBlueprints = decode(fixture).unwrap();
        let bp = &b.blueprints[0];
        assert_eq!(bp.key, "morning-brief");
        assert_eq!(bp.schedule_human, "Every day at 08:00");
        assert_eq!(bp.app_url, "hermes://blueprint/morning-brief");
        assert_eq!(bp.fields[0].field_type, "time");
        assert_eq!(bp.fields[0].default, json!("08:00"));
        assert_eq!(bp.fields[1].options.len(), 3);
        assert_eq!(bp.fields[1].help, None);
        assert_eq!(decode::<CronBlueprints>(json!({})).unwrap(), CronBlueprints::default());
    }

    #[test]
    fn delivery_targets_path_and_decode() {
        assert_eq!(cron_delivery_targets_request("").path, "/api/cron/delivery-targets");
        assert_eq!(
            cron_delivery_targets_request("ea").path,
            "/api/cron/delivery-targets?profile=ea"
        );
        let t: CronDeliveryTargets = decode(json!({"targets": [
            {"id": "local", "name": "Local (save only)", "home_target_set": true, "home_env_var": null},
            {"id": "telegram", "name": "Telegram", "home_target_set": false,
             "home_env_var": "TELEGRAM_HOME_CHANNEL"}
        ]}))
        .unwrap();
        assert_eq!(t.targets[0].id, "local");
        assert_eq!(t.targets[1].home_env_var.as_deref(), Some("TELEGRAM_HOME_CHANNEL"));
        assert!(!t.targets[1].home_target_set);
        assert_eq!(decode::<CronDeliveryTargets>(json!({})).unwrap().targets.len(), 0);
    }

    #[test]
    fn cron_job_paths_and_decode() {
        assert_eq!(cron_job_request("abc", "").path, "/api/cron/jobs/abc");
        assert_eq!(cron_job_request("a b", "swe").path, "/api/cron/jobs/a%20b?profile=swe");

        let r = cron_job_update_request("abc", &json!({"prompt": "new"}), "");
        assert_eq!((r.method, r.path.as_str()), (Method::Put, "/api/cron/jobs/abc"));
        assert_eq!(r.body.unwrap(), json!({"updates": {"prompt": "new"}}));

        let job: CronJob = decode(json!({
            "id": "abc", "name": "Morning", "prompt": "Summarise news", "schedule": "0 9 * * *",
            "schedule_display": "0 9 * * *", "state": "paused", "enabled": false, "paused_at": null,
            "next_run_at": null, "last_run_at": "2026-10-17T09:00:00+05:30", "last_status": null,
            "deliver": "local", "repeat": null, "skills": ["github"], "skill": null, "script": null,
            "latest_execution": {"status": "ok"}
        }))
        .unwrap();
        assert_eq!(job.id, "abc");
        assert_eq!(job.state, "paused");
        assert!(!job.enabled);
        assert_eq!(job.last_status, None);
        assert_eq!(job.deliver.as_deref(), Some("local"));
        assert_eq!(job.skills, ["github"]);
        assert_eq!(decode::<CronJob>(json!({})).unwrap(), CronJob::default());
    }

    #[test]
    fn cron_manage_builders_and_decode() {
        let c = cron_manage_list_call(true, "");
        assert_eq!(c.method, "cron.manage");
        assert_eq!(c.params, json!({"action": "list", "include_disabled": true}));
        assert_eq!(
            cron_manage_list_call(false, "swe").params,
            json!({"action": "list", "include_disabled": false, "profile": "swe"})
        );

        let c = cron_manage_add_call("nightly", "0 3 * * *", "noop", "", None, None, "");
        assert_eq!(
            c.params,
            json!({"action": "add", "name": "nightly", "schedule": "0 3 * * *", "prompt": "noop"})
        );
        let c = cron_manage_add_call("n", "0 3 * * *", "p", "telegram", Some(2), Some(true), "ea");
        assert_eq!(c.params["deliver"], json!("telegram"));
        assert_eq!(c.params["repeat"], json!(2));
        assert_eq!(c.params["continuity"], json!(true));
        assert_eq!(c.params["profile"], json!("ea"));

        let c = cron_manage_job_call("pause", "nightly", "swe");
        assert_eq!(
            c.params,
            json!({"action": "pause", "name": "nightly", "profile": "swe"})
        );

        let ok: CronManageResult = decode(json!({
            "success": true, "count": 1, "scoped": "default",
            "jobs": [{"job_id": "abc", "name": "Morning", "schedule": "0 9 * * *",
                      "prompt_preview": "Summarise", "state": "active", "enabled": true}]
        }))
        .unwrap();
        assert_eq!(ok.count, Some(1));
        assert_eq!(ok.jobs[0].id(), "abc");
        assert_eq!(ok.jobs[0].schedule, "0 9 * * *");
        assert_eq!(ok.scoped.as_deref(), Some("default"));

        let removed: CronManageResult = decode(json!({
            "success": true, "message": "Cron job 'Morning' removed.",
            "removed_job": {"id": "abc", "name": "Morning", "schedule": "0 9 * * *"}
        }))
        .unwrap();
        assert_eq!(removed.removed_job.unwrap().id(), "abc");

        let bad: CronManageResult = decode(json!({"error": "job not found", "success": false})).unwrap();
        assert_eq!(bad.success, Some(false));
        assert_eq!(bad.error.as_deref(), Some("job not found"));
        assert_eq!(
            decode::<CronManageResult>(json!({})).unwrap(),
            CronManageResult::default()
        );
    }

    #[test]
    fn profile_active_description_model_and_describe() {
        let r = profile_active_set_request("swe");
        assert_eq!((r.method, r.path.as_str()), (Method::Post, "/api/profiles/active"));
        assert_eq!(r.body.unwrap(), json!({"name": "swe"}));
        let a: ProfileActiveSet = decode(json!({"ok": true, "active": "swe"})).unwrap();
        assert!(a.ok);
        assert_eq!(a.active, "swe");
        assert_eq!(
            decode::<ProfileActiveSet>(json!({})).unwrap(),
            ProfileActiveSet::default()
        );

        let r = profile_description_set_request("a b", "Writes docs");
        assert_eq!(
            (r.method, r.path.as_str()),
            (Method::Put, "/api/profiles/a%20b/description")
        );
        assert_eq!(r.body.unwrap(), json!({"description": "Writes docs"}));
        let d: ProfileDescriptionSet =
            decode(json!({"ok": true, "description": "Writes docs", "description_auto": false})).unwrap();
        assert_eq!(d.description, "Writes docs");
        assert!(!d.description_auto);

        let r = profile_model_set_request("swe", "openrouter", "anthropic/claude-sonnet-4");
        assert_eq!((r.method, r.path.as_str()), (Method::Put, "/api/profiles/swe/model"));
        assert_eq!(
            r.body.unwrap(),
            json!({"provider": "openrouter", "model": "anthropic/claude-sonnet-4"})
        );
        let m: ProfileModelSet =
            decode(json!({"ok": true, "provider": "openrouter", "model": "anthropic/claude-sonnet-4"})).unwrap();
        assert_eq!(m.model, "anthropic/claude-sonnet-4");

        let r = profile_describe_auto_request("swe", true);
        assert_eq!(
            (r.method, r.path.as_str()),
            (Method::Post, "/api/profiles/swe/describe-auto")
        );
        assert_eq!(r.body.unwrap(), json!({"overwrite": true}));
        let ok: ProfileDescribed =
            decode(json!({"ok": true, "reason": null, "description": "Builds", "description_auto": true})).unwrap();
        assert!(ok.ok);
        assert_eq!(ok.reason, None);
        let bad: ProfileDescribed =
            decode(json!({"ok": false, "reason": "no model configured", "description": "", "description_auto": false}))
                .unwrap();
        assert_eq!(bad.reason.as_deref(), Some("no model configured"));
        assert_eq!(
            decode::<ProfileDescribed>(json!({})).unwrap(),
            ProfileDescribed::default()
        );
    }

    #[test]
    fn profile_export_import_paths_and_decode() {
        let r = profile_export_request("swe", "/tmp/swe.tar.gz", &json!({"desktop.json": "{}"}));
        assert_eq!((r.method, r.path.as_str()), (Method::Post, "/api/profiles/swe/export"));
        assert_eq!(
            r.body.unwrap(),
            json!({"output": "/tmp/swe.tar.gz", "extra_files": {"desktop.json": "{}"}})
        );
        let e: ProfileExported = decode(json!({"ok": true, "archive": "/tmp/swe.tar.gz"})).unwrap();
        assert_eq!(e.archive, "/tmp/swe.tar.gz");

        let r = profile_import_request("/tmp/swe.tar.gz", "");
        assert_eq!((r.method, r.path.as_str()), (Method::Post, "/api/profiles/import"));
        assert_eq!(r.body.unwrap(), json!({"archive": "/tmp/swe.tar.gz"}));
        assert_eq!(
            profile_import_request("a.tar.gz", "research").body.unwrap(),
            json!({"archive": "a.tar.gz", "name": "research"})
        );
        let i: ProfileImported = decode(json!({
            "ok": true, "name": "research", "path": "/h/.hermes/profiles/research",
            "desktop": {"theme": "dark"}
        }))
        .unwrap();
        assert_eq!(i.name, "research");
        assert_eq!(i.desktop, Some(json!({"theme": "dark"})));
        assert_eq!(decode::<ProfileImported>(json!({})).unwrap().desktop, None);
    }

    #[test]
    fn profile_setup_terminal_overlay_paths_and_decode() {
        let r = profile_setup_command_request("swe");
        assert_eq!(
            (r.method, r.path.as_str()),
            (Method::Get, "/api/profiles/swe/setup-command")
        );
        assert_eq!(r.body, None);
        let s: ProfileSetupCommand = decode(json!({"command": "hermes -p swe setup"})).unwrap();
        assert_eq!(s.command, "hermes -p swe setup");

        let r = profile_open_terminal_request("swe");
        assert_eq!(
            (r.method, r.path.as_str()),
            (Method::Post, "/api/profiles/swe/open-terminal")
        );
        let t: ProfileTerminalOpened = decode(json!({"ok": true, "command": "hermes -p swe setup"})).unwrap();
        assert!(t.ok);

        let r = profile_desktop_overlay_request("swe");
        assert_eq!(
            (r.method, r.path.as_str()),
            (Method::Get, "/api/profiles/swe/desktop-overlay")
        );
        let o: ProfileDesktopOverlay = decode(json!({"exists": true, "desktop": {"appearance": "light"}})).unwrap();
        assert!(o.exists);
        assert_eq!(o.desktop, Some(json!({"appearance": "light"})));
        let missing: ProfileDesktopOverlay = decode(json!({"exists": false, "desktop": null})).unwrap();
        assert!(!missing.exists);
        assert_eq!(missing.desktop, None);
        assert_eq!(
            decode::<ProfileDesktopOverlay>(json!({})).unwrap(),
            ProfileDesktopOverlay::default()
        );
    }

    #[test]
    fn profile_rpcs_builders_and_decode() {
        assert_eq!(profiles_list_call(true).params, json!({"include_sessions": true}));
        let list: ProfileList = decode(json!({
            "profiles": [{
                "name": "default", "path": "/h/.hermes", "is_default": true, "model": null,
                "provider": null, "description": "", "display_name": "", "skill_count": 3,
                "previous_names": ["main"], "ui_meta": {"accent": "amber"},
                "ui_meta_revisions": {"ui_meta": 2}, "has_avatar": false,
                "last_session": {"id": "s1"}, "worker_session": null,
                "canonical_session": {"id": "s1"}
            }],
            "bot_mode_protocol": true, "install_id": "abc123"
        }))
        .unwrap();
        assert_eq!(list.install_id, "abc123");
        assert_eq!(list.profiles[0].skill_count, 3);
        assert_eq!(list.profiles[0].model, None);
        assert_eq!(list.profiles[0].previous_names, ["main"]);
        assert_eq!(list.profiles[0].ui_meta, Some(json!({"accent": "amber"})));
        assert_eq!(list.profiles[0].worker_session, None);
        assert!(list.bot_mode_protocol);
        assert_eq!(decode::<ProfileList>(json!({})).unwrap(), ProfileList::default());

        assert_eq!(profile_describe_call("swe").params, json!({"name": "swe"}));
        let snap: ProfileSnapshot = decode(json!({
            "name": "swe", "description": "d", "soul": "# Soul",
            "model": {"provider": "openrouter", "default": "m1"},
            "skills": [{"name": "github", "enabled": true}],
            "toolsets": [{"name": "web", "label": "Web", "description": "", "enabled": true}],
            "toolsets_pinned": false,
            "mcp_servers": [{"name": "fs", "enabled": true, "transport": "stdio"}]
        }))
        .unwrap();
        assert_eq!(snap.model.default, "m1");
        assert_eq!(snap.skills[0].name, "github");
        assert_eq!(snap.toolsets[0].enabled, Some(true));
        assert_eq!(snap.mcp_servers[0].transport, "stdio");
        assert!(!snap.toolsets_pinned);

        let c = profile_configure_call("swe", &json!({"description": "d", "model": "m1", "provider": "p"}));
        assert_eq!(
            c.params,
            json!({"description": "d", "model": "m1", "provider": "p", "name": "swe"})
        );
        assert_eq!(
            profile_configure_call("swe", &json!("nope")).params,
            json!({"name": "swe"})
        );
        let applied: ProfileConfigured = decode(json!({
            "ok": true, "applied": {"description": true},
            "confirm_required": true, "confirm_message": "This model is expensive"
        }))
        .unwrap();
        assert!(applied.ok);
        assert_eq!(applied.applied["description"], json!(true));
        assert_eq!(applied.confirm_message.as_deref(), Some("This model is expensive"));
        assert_eq!(
            decode::<ProfileConfigured>(json!({})).unwrap(),
            ProfileConfigured::default()
        );
    }

    #[test]
    fn profile_asset_calls_and_decode() {
        let c = profiles_set_asset_call("swe", "avatar", "AAAA", false);
        assert_eq!(c.params, json!({"name": "swe", "asset": "avatar", "data": "AAAA"}));
        let clear = profiles_set_asset_call("swe", "avatar", "", true);
        assert_eq!(clear.params, json!({"name": "swe", "asset": "avatar", "clear": true}));

        let set: ProfileAssetSet = decode(json!({"ok": true, "asset": "avatar", "size": 68})).unwrap();
        assert_eq!(set.size, Some(68));
        assert_eq!(set.removed, None);
        let removed: ProfileAssetSet = decode(json!({"ok": true, "asset": "avatar", "size": 0, "removed": 1})).unwrap();
        assert_eq!(removed.removed, Some(1));

        assert_eq!(
            profiles_get_asset_call("swe", "avatar").params,
            json!({"name": "swe", "asset": "avatar"})
        );
        let found: ProfileAssetData = decode(json!({
            "found": true, "mime": "image/png", "size": 68, "data": "data:image/png;base64,AAAA"
        }))
        .unwrap();
        assert!(found.found);
        assert!(found.data.starts_with("data:image/png;base64,"));
        let none: ProfileAssetData = decode(json!({"found": false})).unwrap();
        assert!(!none.found);
        assert_eq!(none.data, "");
        assert_eq!(
            decode::<ProfileAssetData>(json!({})).unwrap(),
            ProfileAssetData::default()
        );
    }

    #[test]
    fn memory_provider_config_paths_and_decode() {
        assert_eq!(
            memory_provider_config_request("hindsight", "declared", "").path,
            "/api/memory/providers/hindsight/config?surface=declared"
        );
        assert_eq!(
            memory_provider_config_request("a b", "", "swe").path,
            "/api/memory/providers/a%20b/config?profile=swe"
        );
        assert_eq!(
            memory_provider_config_request("x", "declared", "swe").path,
            "/api/memory/providers/x/config?surface=declared&profile=swe"
        );

        let cfg: MemoryProviderConfig = decode(json!({
            "name": "hindsight", "label": "Hindsight", "docs_url": "https://docs.example/h",
            "fields": [
                {"key": "api_key", "label": "API key", "kind": "secret", "description": "Token",
                 "info": null, "placeholder": "sk-…", "inline": false, "group": "auth",
                 "options": [], "value": "", "is_set": true},
                {"key": "mode", "label": "Mode", "kind": "select", "description": null,
                 "info": "Pick one", "placeholder": null, "inline": true, "group": null,
                 "options": [{"value": "fast", "label": "Fast", "description": "Less accurate"}],
                 "value": "fast", "is_set": true}
            ]
        }))
        .unwrap();
        assert_eq!(cfg.name, "hindsight");
        assert_eq!(cfg.docs_url, "https://docs.example/h");
        assert_eq!(cfg.fields[0].kind, "secret");
        assert_eq!(cfg.fields[0].value, json!(""));
        assert_eq!(cfg.fields[0].is_set, Some(true));
        assert_eq!(cfg.fields[1].options[0].value, "fast");
        assert_eq!(cfg.fields[1].group, None);

        let legacy: MemoryProviderConfig = decode(json!({
            "name": "byterover", "label": "Byterover",
            "fields": [{"key": "token", "label": "Token", "kind": "secret", "description": "",
                        "placeholder": "", "required": true, "value": "", "is_set": false,
                        "options": [], "url": ""}],
            "setup": {"install": "pip install byterover"}
        }))
        .unwrap();
        assert_eq!(legacy.fields[0].required, Some(true));
        assert!(legacy.setup.is_some());
        assert_eq!(decode::<MemoryProviderConfig>(json!({})).unwrap().fields.len(), 0);
    }

    #[test]
    fn memory_provider_config_update_setup_and_oauth() {
        let r = memory_provider_config_update_request("x", &json!({"k": "v"}), "declared", "");
        assert_eq!(
            (r.method, r.path.as_str()),
            (Method::Put, "/api/memory/providers/x/config?surface=declared")
        );
        assert_eq!(r.body.unwrap(), json!({"values": {"k": "v"}}));
        let u: MemoryProviderConfigUpdated = decode(json!({"ok": true, "active": "x"})).unwrap();
        assert_eq!(u.active.as_deref(), Some("x"));
        let u2: MemoryProviderConfigUpdated = decode(json!({"ok": true})).unwrap();
        assert_eq!(u2.active, None);
        assert_eq!(
            decode::<MemoryProviderConfigUpdated>(json!({})).unwrap(),
            MemoryProviderConfigUpdated::default()
        );

        let r = memory_provider_setup_request("hindsight", None, "");
        assert_eq!(
            (r.method, r.path.as_str()),
            (Method::Post, "/api/memory/providers/hindsight/setup")
        );
        assert_eq!(r.body.unwrap(), json!({}));
        let r = memory_provider_setup_request("h", Some(&json!({"api_key": "k"})), "swe");
        assert_eq!(r.path, "/api/memory/providers/h/setup?profile=swe");
        assert_eq!(r.body.unwrap(), json!({"values": {"api_key": "k"}}));
        let s: MemorySetupResult = decode(json!({
            "ok": true, "provider": "hindsight",
            "results": [{"kind": "pip", "name": "hindsight", "status": "installed",
                         "command": "hermes pm install", "returncode": 0, "stdout": "ok", "stderr": ""}],
            "status": {"name": "hindsight", "available": true}
        }))
        .unwrap();
        assert!(s.ok);
        assert_eq!(s.results[0].returncode, Some(0));
        assert_eq!(s.status, Some(json!({"name": "hindsight", "available": true})));
        assert_eq!(
            decode::<MemorySetupResult>(json!({})).unwrap(),
            MemorySetupResult::default()
        );

        let r = memory_provider_oauth_start_request("hindsight", "swe");
        assert_eq!(
            (r.method, r.path.as_str()),
            (Method::Post, "/api/memory/providers/hindsight/oauth/start?profile=swe")
        );
        let r = memory_provider_oauth_status_request("hindsight", "");
        assert_eq!(
            (r.method, r.path.as_str()),
            (Method::Get, "/api/memory/providers/hindsight/oauth/status")
        );
        let st: MemoryOAuthStatus = decode(json!({"status": "pending", "error": null, "detail": "Waiting"})).unwrap();
        assert_eq!(st.status, "pending");
        assert_eq!(st.error, None);
        assert_eq!(
            decode::<MemoryOAuthStatus>(json!({})).unwrap(),
            MemoryOAuthStatus::default()
        );
    }

    #[test]
    fn profiles_create_rpc() {
        let mut opts = ProfileCreateOptions {
            description: "Test".into(),
            clone_from: "default".into(),
            clone_all: true,
            soul: "SOUL".into(),
            ..ProfileCreateOptions::default()
        };
        assert_eq!(
            profiles_create_call("work", &opts, None).params,
            json!({"name": "work", "description": "Test", "clone_from": "default",
                   "soul": "SOUL", "clone_all": true})
        );
        opts.clone_from.clear();
        assert_eq!(
            profiles_create_call("work", &opts, Some(false)).params,
            json!({"name": "work", "description": "Test", "soul": "SOUL", "clone_all": true,
                   "mirror_credentials": false})
        );
        let c: ProfileCreated = decode(json!({
            "ok": true, "name": "work", "path": "/h/profiles/work", "soul_written": true,
            "model_set": false, "mirrored": {"auth": true, "env": false}
        }))
        .unwrap();
        assert!(c.ok && c.soul_written);
        assert_eq!(c.mirrored["auth"], json!(true));
        assert_eq!(decode::<ProfileCreated>(json!({})).unwrap(), ProfileCreated::default());
    }
}
