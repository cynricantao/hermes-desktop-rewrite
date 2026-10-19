//! Models and providers: `/api/model/*`, `/api/providers/*` and `/api/local-models/*` REST
//! plus the `model.options`, `model.disconnect` and `model.save_key` JSON-RPCs.
//!
//! Routes, bodies and reply fields mirror `hermes_cli/web_routers/models.py`,
//! `web_routers/oauth.py`, `web_routers/config_env.py` (providers/validate +
//! custom-endpoints), `web_routers/local_models.py`, `hermes_cli/web_models.py` and the
//! `model.*` handlers in `tui_gateway/methods_complete.py`.
//!
//! # Two shapes of "model options"
//! `GET /api/model/options` and the `model.options` RPC answer the same payload
//! ([`ModelOptionsResult`] from [`crate::protocol`], re-exported here), so the Models page
//! and the picker share one type. `model.save_key` answers one refreshed provider row of
//! that same shape ([`ModelOptionProvider`]).
//!
//! # Profile scope
//! Every route below takes `profile: &str` (`""` = default) through [`profile_query`]. The
//! backend resolves config per profile, so the Models page must read the SAME profile the
//! write targets. `model.disconnect` / `model.save_key` carry the gateway's `session_id`
//! instead: the gateway binds them to the profile that owns the session.
//!
//! # Safety
//! Read-only: `model/info`, `model/options`, `model/auxiliary`, `model/moa` (GET),
//! `model/recommended-default`, `providers/oauth` (GET), `providers/custom-endpoints` (GET),
//! `local-models/{hardware,jobs,jobs/{id}}`, and `local-models/status` (which may move a
//! legacy models folder into place once). `local-models/catalog` also starts a TTL-gated
//! background **network** refresh, so do not poll it. Everything else **writes config,
//! contacts a provider, downloads, spawns or deletes** and is documented individually — the
//! UI must confirm first. `local-models/search*` reach Hugging Face; `providers/validate` and the OAuth routes reach the provider itself.

use std::collections::BTreeMap;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::api::{ApiError, Call, Method, Request, call_as, call_of, decode, profile_query, req, seg, with_query};
use crate::http::Rest;
use crate::rpc::Client;

pub use crate::protocol::{ModelOption, ModelOptionProvider, ModelOptionsResult};

/// `GET /api/model/info` bounds its context-length probe at 5 s server-side; the desktop
/// uses the same budget (`MODEL_INFO_REQUEST_TIMEOUT_MS`) so an unreachable `model.base_url`
/// cannot hold the settings page.
pub const MODEL_INFO_TIMEOUT: Duration = Duration::from_secs(5);

/// Send a prepared [`Request`] with an explicit timeout and decode the reply.
async fn send_timed_as<T: DeserializeOwned>(rest: &Rest, r: &Request, timeout: Duration) -> Result<T, ApiError> {
    let v = rest
        .send_with_timeout(r.method.as_str(), &r.path, r.body.as_ref(), timeout)
        .await?;
    decode(v)
}

/// A route's `?flag=1` value (the backend parses `bool` query params, `1` is truthy).
fn flag(on: bool) -> &'static str {
    if on { "1" } else { "" }
}

// ════════════════════════════════════════════════════════════════════════════════
// GET /api/model/info
// ════════════════════════════════════════════════════════════════════════════════

/// models.dev capabilities when the catalog knows the model. Every field is optional because
/// the backend sends `capabilities: {}` when the lookup fails or the model is unknown.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ModelCapabilities {
    pub supports_tools: Option<bool>,
    pub supports_vision: Option<bool>,
    pub supports_reasoning: Option<bool>,
    pub context_window: Option<i64>,
    pub max_output_tokens: Option<i64>,
    pub model_family: Option<String>,
}

/// Resolved metadata for the configured model. `auto_context_length` is what the backend
/// detected (`0` = unknown, including when the 5 s probe budget ran out);
/// `config_context_length` is the user's override (`0` = unset); `effective_context_length`
/// is what the agent actually uses (`config` if set, else `auto`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ModelInfo {
    pub model: String,
    pub provider: String,
    pub auto_context_length: i64,
    pub config_context_length: i64,
    pub effective_context_length: i64,
    pub capabilities: ModelCapabilities,
}

/// Read the configured model's resolved metadata. Read-only, but the backend probes the
/// provider's `/models` endpoint (bounded at [`MODEL_INFO_TIMEOUT`]) when the context length
/// is not cached — an offline provider degrades to `auto_context_length: 0`.
pub fn model_info_request(profile: &str) -> Request {
    req(Method::Get, format!("/api/model/info{}", profile_query(profile)), None)
}

/// Fetch the resolved model info (see [`model_info_request`]).
pub async fn model_info(rest: &Rest, profile: &str) -> Result<ModelInfo, ApiError> {
    send_timed_as(rest, &model_info_request(profile), MODEL_INFO_TIMEOUT).await
}

// ════════════════════════════════════════════════════════════════════════════════
// GET /api/model/options
// ════════════════════════════════════════════════════════════════════════════════

/// Which providers/models the picker offers.
///
/// * `refresh` busts the per-provider model-id disk cache (the picker's "Refresh Models");
///   the normal open stays on the backend's 1 h cache and can block ~15 s on a models.dev
///   cache miss, so give the call a generous timeout.
/// * `include_unconfigured` also lists providers with no credential.
/// * `explicit_only` restricts the list to explicitly configured providers/models.
pub fn model_options_request(profile: &str, refresh: bool, include_unconfigured: bool, explicit_only: bool) -> Request {
    let path = with_query(
        "/api/model/options",
        &[
            ("refresh", flag(refresh)),
            ("include_unconfigured", flag(include_unconfigured)),
            ("explicit_only", flag(explicit_only)),
            ("profile", if profile.is_empty() { "" } else { profile }),
        ],
    );
    req(Method::Get, path, None)
}

/// List provider/model options for the picker (see [`model_options_request`]).
pub async fn model_options(
    rest: &Rest,
    profile: &str,
    refresh: bool,
    include_unconfigured: bool,
    explicit_only: bool,
) -> Result<ModelOptionsResult, ApiError> {
    crate::api::send_as(
        rest,
        &model_options_request(profile, refresh, include_unconfigured, explicit_only),
    )
    .await
}

/// `model.options` RPC — same payload as [`model_options`], bound to a session so the
/// gateway overlays the live agent's provider when one is running.
pub fn model_options_call(
    profile: &str,
    refresh: bool,
    include_unconfigured: bool,
    explicit_only: bool,
    session_id: &str,
) -> Call {
    let mut params = json!({
        "explicit_only": explicit_only,
        "include_unconfigured": include_unconfigured,
        "refresh": refresh,
    });
    if !profile.is_empty() {
        params["profile"] = json!(profile);
    }
    if !session_id.is_empty() {
        params["session_id"] = json!(session_id);
    }
    call_of("model.options", params)
}

/// `model.options` over the gateway (see [`model_options_call`]).
pub async fn model_options_rpc(
    client: &Client,
    profile: &str,
    refresh: bool,
    include_unconfigured: bool,
    explicit_only: bool,
    session_id: &str,
) -> Result<ModelOptionsResult, ApiError> {
    call_as(
        client,
        &model_options_call(profile, refresh, include_unconfigured, explicit_only, session_id),
    )
    .await
}

// ════════════════════════════════════════════════════════════════════════════════
// POST /api/model/set
// ════════════════════════════════════════════════════════════════════════════════

/// Which slot an assignment targets. `Auxiliary` with an empty `task` writes **every**
/// auxiliary slot; the sentinel `"__reset__"` resets every slot to `provider: "auto"`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ModelScope {
    #[default]
    Main,
    Auxiliary,
}

impl ModelScope {
    pub fn as_str(self) -> &'static str {
        match self {
            ModelScope::Main => "main",
            ModelScope::Auxiliary => "auxiliary",
        }
    }
}

/// `POST /api/model/set` body (`ModelAssignment` in `hermes_cli/web_models.py`).
///
/// `reasoning_effort` keeps the backend's three-way meaning: `None` leaves the task's
/// override alone, `Some(None)` clears it (inherit the main agent's effort), and
/// `Some(Some(level))` sets it.
#[derive(Clone, Default, PartialEq)]
pub struct ModelAssignment {
    pub scope: ModelScope,
    pub provider: String,
    pub model: String,
    pub task: String,
    pub reasoning_effort: Option<Option<String>>,
    pub base_url: String,
    pub api_key: String,
    pub confirm_expensive_model: bool,
    pub profile: String,
}

impl ModelAssignment {
    /// Assign the main conversation model.
    pub fn main(provider: &str, model: &str) -> Self {
        Self {
            scope: ModelScope::Main,
            provider: provider.into(),
            model: model.into(),
            ..Self::default()
        }
    }

    /// Assign one auxiliary task slot (`task` empty = all slots, `"__reset__"` = reset all).
    pub fn auxiliary(task: &str, provider: &str, model: &str) -> Self {
        Self {
            scope: ModelScope::Auxiliary,
            task: task.into(),
            provider: provider.into(),
            model: model.into(),
            ..Self::default()
        }
    }

    pub fn with_base_url(mut self, base_url: &str) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// **Secret**: written to the profile's config/.env by the backend.
    pub fn with_api_key(mut self, api_key: &str) -> Self {
        self.api_key = api_key.into();
        self
    }

    /// Acknowledge the expensive-model warning so the backend applies the write.
    pub fn confirm_expensive(mut self) -> Self {
        self.confirm_expensive_model = true;
        self
    }

    /// `Some(None)` clears the override, `Some(Some(level))` sets it, `None` leaves it.
    pub fn with_reasoning_effort(mut self, effort: Option<&str>) -> Self {
        self.reasoning_effort = Some(effort.map(str::to_owned));
        self
    }

    fn to_body(&self) -> Value {
        let mut body = json!({
            "scope": self.scope.as_str(),
            "provider": self.provider,
            "model": self.model,
            "task": self.task,
            "base_url": self.base_url,
            "api_key": self.api_key,
            "confirm_expensive_model": self.confirm_expensive_model,
        });
        if let Some(effort) = &self.reasoning_effort {
            body["reasoning_effort"] = json!(effort);
        }
        if !self.profile.is_empty() {
            body["profile"] = json!(self.profile);
        }
        body
    }
}

/// The assignment reply. `ok: false` + `confirm_required: true` is NOT an error: the model
/// looks expensive, the backend has not written anything, and the UI must show
/// `confirm_message` and repeat the call with `confirm_expensive_model: true`
/// ([`ModelAssignment::confirm_expensive`]).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ModelAssignmentReply {
    pub ok: bool,
    pub scope: String,
    pub provider: String,
    pub model: String,
    /// Auxiliary saves: the task slots written (`__all__` expands to every task).
    pub tasks: Vec<String>,
    /// Auxiliary `__reset__`: every task slot went back to `auto`.
    pub reset: bool,
    /// Main saves: the endpoint now in force and the auxiliary slots left on the old one.
    pub base_url: Option<String>,
    pub stale_aux: Option<Value>,
    pub confirm_required: bool,
    pub confirm_message: Option<String>,
}

/// Assign a model to the main slot or an auxiliary task. **Writes the profile's
/// `config.yaml`** — new sessions only; a running chat hot-swaps via `/model` instead.
/// May answer `confirm_required` (see [`ModelAssignmentReply`]).
pub fn model_set_request(profile: &str, assignment: &ModelAssignment) -> Request {
    req(
        Method::Post,
        format!("/api/model/set{}", profile_query(profile)),
        Some(assignment.to_body()),
    )
}

/// Save a model assignment (see [`model_set_request`]).
pub async fn model_set(
    rest: &Rest,
    profile: &str,
    assignment: &ModelAssignment,
) -> Result<ModelAssignmentReply, ApiError> {
    crate::api::send_as(rest, &model_set_request(profile, assignment)).await
}

// ════════════════════════════════════════════════════════════════════════════════
// GET /api/model/auxiliary
// ════════════════════════════════════════════════════════════════════════════════

/// The resolved route an inheriting plugin task points at right now.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct AuxiliaryEffective {
    pub provider: String,
    pub model: String,
    pub base_url: String,
}

/// One auxiliary task slot. Built-in slots come first with no `label`/`plugin` (the client
/// labels them); plugin-registered tasks follow and carry `label`, `hint`, `plugin` and
/// `inherit_from` (base slot key). An inheriting row also carries `effective` while it is
/// unpinned. `provider`/`model`/`base_url` are always the slot's OWN stored values.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct AuxiliaryTask {
    pub task: String,
    /// `"auto"` when the slot is unpinned.
    pub provider: String,
    pub model: String,
    pub base_url: String,
    pub reasoning_effort: Option<String>,
    /// A free local/LAN endpoint rather than a paid provider — lets the UI tell them apart.
    pub local_endpoint: bool,
    pub label: Option<String>,
    pub hint: Option<String>,
    pub plugin: Option<String>,
    pub inherit_from: Option<String>,
    pub effective: Option<AuxiliaryEffective>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct MainModelSlot {
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct AuxiliaryModels {
    pub tasks: Vec<AuxiliaryTask>,
    pub main: MainModelSlot,
}

/// Read the auxiliary task assignments. Read-only, config only.
pub fn auxiliary_models_request(profile: &str) -> Request {
    req(
        Method::Get,
        format!("/api/model/auxiliary{}", profile_query(profile)),
        None,
    )
}

/// Fetch the auxiliary assignments (see [`auxiliary_models_request`]).
pub async fn auxiliary_models(rest: &Rest, profile: &str) -> Result<AuxiliaryModels, ApiError> {
    crate::api::send_as(rest, &auxiliary_models_request(profile)).await
}

// ════════════════════════════════════════════════════════════════════════════════
// GET | PUT /api/model/moa
// ════════════════════════════════════════════════════════════════════════════════

/// One Mixture-of-Agents provider/model slot. `enabled` defaults to true (a slot the user
/// disabled stays in the config but is skipped).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct MoaSlot {
    pub provider: String,
    pub model: String,
    /// Declared so a GET → PUT round-trip does not strip it.
    pub reasoning_effort: Option<String>,
    pub enabled: bool,
}

/// One named MoA preset. `reference_timeout: null` inherits
/// `auxiliary.moa_reference.timeout` (900 s default). Temperatures `null` mean "omit from
/// the provider call", exactly like a single model with no temperature.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct MoaPreset {
    pub reference_models: Vec<MoaSlot>,
    pub aggregator: MoaSlot,
    pub reference_temperature: Option<f64>,
    pub aggregator_temperature: Option<f64>,
    pub reference_timeout: Option<f64>,
    /// `"loud"` (warn when a reference fails) | `"silent"`.
    pub degraded_reference_policy: String,
    /// Fan-out strategy name, when the backend pins one.
    pub fanout: Option<String>,
    pub enabled: bool,
}

/// The normalized MoA config the backend both returns and accepts. GET answers the flat
/// compatibility view (`reference_models`/`aggregator`/…) of the default preset alongside
/// `presets`; PUT applies the same object and answers `{"ok": true, …normalized}`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct MoaConfig {
    pub default_preset: String,
    /// `""` when no preset is active (MoA off).
    pub active_preset: String,
    pub presets: BTreeMap<String, MoaPreset>,
    pub reference_models: Vec<MoaSlot>,
    pub aggregator: MoaSlot,
    pub reference_temperature: Option<f64>,
    pub aggregator_temperature: Option<f64>,
    pub reference_timeout: Option<f64>,
    pub degraded_reference_policy: String,
    pub fanout: Option<String>,
    pub enabled: bool,
    /// Modes are backend-defined (`coerce_privacy_filter`); free-form on purpose.
    pub privacy_filter: Option<Value>,
}

/// Read the configured MoA slots. Read-only, config only.
pub fn moa_request(profile: &str) -> Request {
    req(Method::Get, format!("/api/model/moa{}", profile_query(profile)), None)
}

/// Fetch the MoA config (see [`moa_request`]).
pub async fn moa(rest: &Rest, profile: &str) -> Result<MoaConfig, ApiError> {
    crate::api::send_as(rest, &moa_request(profile)).await
}

/// Persist the MoA slots. **Writes `config.yaml`.** The backend validates strictly and
/// answers 422 (`Invalid MoA config: …`) rather than silently repairing a half-filled slot —
/// so a preset with an empty provider/model must be dropped before the PUT. Unknown keys
/// inside a preset are rejected too; `privacy_filter` is preserved server-side.
pub fn moa_set_request(profile: &str, config: &MoaConfig) -> Request {
    req(
        Method::Put,
        format!("/api/model/moa{}", profile_query(profile)),
        Some(serde_json::to_value(config).unwrap_or(Value::Null)),
    )
}

/// Save the MoA config (see [`moa_set_request`]).
pub async fn moa_set(rest: &Rest, profile: &str, config: &MoaConfig) -> Result<MoaConfig, ApiError> {
    crate::api::send_as(rest, &moa_set_request(profile, config)).await
}

// ════════════════════════════════════════════════════════════════════════════════
// GET /api/model/recommended-default
// ════════════════════════════════════════════════════════════════════════════════

/// The default a freshly-authenticated provider should land on. `free_tier` is only set for
/// `nous` (`true` free / `false` paid); `model` may be empty, which the caller must tolerate
/// (fall back to the provider's first curated row, or leave the picker empty).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct RecommendedDefault {
    pub provider: String,
    pub model: String,
    pub free_tier: Option<bool>,
}

/// Ask which model a provider should default to. Read-only, but the backend may build the
/// provider inventory (cache reads) and, for a custom provider, persist discovered model ids
/// into `config.yaml` on the way.
pub fn recommended_default_request(profile: &str, provider: &str) -> Request {
    let path = with_query(
        "/api/model/recommended-default",
        &[("provider", provider), ("profile", profile)],
    );
    req(Method::Get, path, None)
}

/// Fetch the recommended default (see [`recommended_default_request`]).
pub async fn recommended_default(rest: &Rest, profile: &str, provider: &str) -> Result<RecommendedDefault, ApiError> {
    crate::api::send_as(rest, &recommended_default_request(profile, provider)).await
}

// ════════════════════════════════════════════════════════════════════════════════
// /api/providers/oauth
// ════════════════════════════════════════════════════════════════════════════════

/// Login state of one OAuth provider card. **`token_preview` is the truncated tail of a
/// token** — never log it, never show more than the backend already truncated.
#[derive(Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct OAuthStatus {
    pub logged_in: bool,
    /// `nous_portal` | `api_key` | `env_var` | `openai_codex` | …
    pub source: Option<String>,
    pub source_label: Option<String>,
    pub token_preview: Option<String>,
    /// ISO-8601 string for most providers, epoch seconds for a few — free-form on purpose.
    pub expires_at: Option<Value>,
    pub has_refresh_token: Option<bool>,
    pub last_refresh: Option<Value>,
    /// Nous only: free-tier identity.
    pub free_tier: Option<bool>,
    pub account_tier: Option<String>,
    /// Set when status resolution itself failed (`{"logged_in": false, "error": …}`).
    pub error: Option<String>,
}

/// One OAuth-capable provider. `flow: "external"` providers log in through their own CLI
/// (`cli_command`); only `device_code` providers can start a flow here.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct OAuthProvider {
    pub id: String,
    pub name: String,
    pub flow: String,
    pub cli_command: Option<String>,
    pub docs_url: Option<String>,
    /// Why the API cannot clear this provider, when it cannot.
    pub disconnect_hint: Option<String>,
    /// Shell command that clears an external provider the API will not touch (Claude Code).
    pub disconnect_command: Option<String>,
    /// True when [`oauth_disconnect_request`] may be used.
    pub disconnectable: bool,
    pub status: OAuthStatus,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct OAuthProviders {
    pub providers: Vec<OAuthProvider>,
}

/// List OAuth providers with their login status. Read-only (local credential stores; Nous
/// uses a refresh-free local snapshot, so no OAuth refresh happens here).
pub fn oauth_providers_request(profile: &str) -> Request {
    req(
        Method::Get,
        format!("/api/providers/oauth{}", profile_query(profile)),
        None,
    )
}

/// List OAuth providers (see [`oauth_providers_request`]).
pub async fn oauth_providers(rest: &Rest, profile: &str) -> Result<OAuthProviders, ApiError> {
    crate::api::send_as(rest, &oauth_providers_request(profile)).await
}

/// The device-code display fields the UI renders while it polls.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct OAuthStart {
    pub session_id: String,
    /// Always `"device_code"` for a successful start.
    pub flow: String,
    pub user_code: String,
    /// Where the user types the code (Nous/Codex: a complete URL).
    pub verification_url: String,
    pub expires_in: i64,
    /// Seconds between polls.
    pub poll_interval: i64,
}

/// Start an OAuth device-code login. **Network + writes credentials**: the backend calls the
/// provider's device-auth endpoint and spawns a poller thread; when the poll later reports
/// `approved` the tokens are already saved. `flow: "external"` providers answer 400 — show
/// `cli_command` instead. Opening `verification_url` in a browser is the user's step.
pub fn oauth_start_request(profile: &str, provider_id: &str) -> Request {
    req(
        Method::Post,
        format!(
            "/api/providers/oauth/{}/start{}",
            seg(provider_id),
            profile_query(profile)
        ),
        None,
    )
}

/// Start a device-code login (see [`oauth_start_request`]).
pub async fn oauth_start(rest: &Rest, profile: &str, provider_id: &str) -> Result<OAuthStart, ApiError> {
    crate::api::send_as(rest, &oauth_start_request(profile, provider_id)).await
}

/// One poll of a login session. `status` is `pending` | `approved` | `expired` | `error`
/// (the backend's own vocabulary); `reason`/`account_email`/`model` describe a Nous
/// free-tier transfer. Poll `approved` → the credentials are saved; stop polling otherwise.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct OAuthPoll {
    pub session_id: String,
    pub status: String,
    pub error_message: Option<String>,
    pub expires_at: Option<f64>,
    pub reason: Option<String>,
    pub account_email: Option<String>,
    pub model: Option<String>,
    pub retryable: Option<bool>,
    pub retry_after: Option<f64>,
}

/// Poll a login session. Read-only local state; no provider call. 404 once the session
/// expired or was cancelled — treat that as "start over", not as a hard error.
pub fn oauth_poll_request(profile: &str, provider_id: &str, session_id: &str) -> Request {
    req(
        Method::Get,
        format!(
            "/api/providers/oauth/{}/poll/{}{}",
            seg(provider_id),
            seg(session_id),
            profile_query(profile)
        ),
        None,
    )
}

/// Poll a login session (see [`oauth_poll_request`]).
pub async fn oauth_poll(
    rest: &Rest,
    profile: &str,
    provider_id: &str,
    session_id: &str,
) -> Result<OAuthPoll, ApiError> {
    crate::api::send_as(rest, &oauth_poll_request(profile, provider_id, session_id)).await
}

/// Cancel a pending login. `ok: false` + `message: "session not found"` when it already
/// ended — not an error.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct OAuthCancel {
    pub ok: bool,
    pub message: Option<String>,
    pub session_id: Option<String>,
}

/// Abort a login session. **Stops the backend's poller**: the provider will not be connected
/// even if the user finishes the browser step afterwards.
pub fn oauth_cancel_request(profile: &str, session_id: &str) -> Request {
    req(
        Method::Delete,
        format!(
            "/api/providers/oauth/sessions/{}{}",
            seg(session_id),
            profile_query(profile)
        ),
        None,
    )
}

/// Cancel a login session (see [`oauth_cancel_request`]).
pub async fn oauth_cancel(rest: &Rest, profile: &str, session_id: &str) -> Result<OAuthCancel, ApiError> {
    crate::api::send_as(rest, &oauth_cancel_request(profile, session_id)).await
}

/// Submit an authorization code for a PKCE flow. **Currently refused by the backend**: every
/// provider uses device-code or an external CLI, so this route always answers 400
/// (`submit not supported for <provider>`). Kept for parity and for a future PKCE provider.
pub fn oauth_submit_request(profile: &str, provider_id: &str, session_id: &str, code: &str) -> Request {
    req(
        Method::Post,
        format!(
            "/api/providers/oauth/{}/submit{}",
            seg(provider_id),
            profile_query(profile)
        ),
        Some(json!({ "session_id": session_id, "code": code })),
    )
}

/// Submit an auth code (see [`oauth_submit_request`]; expect HTTP 400 today).
pub async fn oauth_submit(
    rest: &Rest,
    profile: &str,
    provider_id: &str,
    session_id: &str,
    code: &str,
) -> Result<Value, ApiError> {
    crate::api::send(rest, &oauth_submit_request(profile, provider_id, session_id, code)).await
}

/// Disconnect an OAuth provider. 409 when nothing was stored; 400 for an external provider
/// (run `disconnect_command` instead).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct OAuthDisconnect {
    pub ok: bool,
    pub provider: String,
}

/// Remove a provider's stored credentials. **Destructive**: deletes the OAuth grant / PKCE
/// file / credential-pool entry. The user must re-authenticate to use that provider again.
pub fn oauth_disconnect_request(profile: &str, provider_id: &str) -> Request {
    req(
        Method::Delete,
        format!("/api/providers/oauth/{}{}", seg(provider_id), profile_query(profile)),
        None,
    )
}

/// Disconnect a provider (see [`oauth_disconnect_request`]).
pub async fn oauth_disconnect(rest: &Rest, profile: &str, provider_id: &str) -> Result<OAuthDisconnect, ApiError> {
    crate::api::send_as(rest, &oauth_disconnect_request(profile, provider_id)).await
}

// ════════════════════════════════════════════════════════════════════════════════
// POST /api/providers/validate
// ════════════════════════════════════════════════════════════════════════════════

/// A credential pre-check before saving it (`EnvVarUpdate`). `key` is the env var name
/// (`OPENAI_API_KEY`, `GEMINI_API_KEY`, …). Special case: `key: "OPENAI_BASE_URL"` validates
/// a custom endpoint's connectivity and lists its model ids; `api_key` is the optional bearer
/// for that probe.
#[derive(Clone, Default, PartialEq)]
pub struct CredentialProbe {
    pub key: String,
    pub value: String,
    pub api_key: String,
    pub profile: String,
    pub provider_setup: bool,
}

impl CredentialProbe {
    /// Probe one env var's value. **`value` is a secret** — never log it.
    pub fn new(key: &str, value: &str) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
            ..Self::default()
        }
    }

    /// Bearer for the `OPENAI_BASE_URL` probe.
    pub fn with_api_key(mut self, api_key: &str) -> Self {
        self.api_key = api_key.into();
        self
    }
}

/// One model id a probed endpoint advertises, with the routing metadata a gateway may attach
/// (`gpt-5.6-sol-high` → `canonical_model: gpt-5.6-sol`, `reasoning_effort: high`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct EndpointModelDetail {
    pub id: String,
    pub canonical_model: Option<String>,
    pub reasoning_effort: Option<String>,
}

/// A validation verdict. `ok: false` + `reachable: true` = the key is bad (block the save);
/// `reachable: false` = the probe could not run (offline — save with a warning instead).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct CredentialValidation {
    pub ok: bool,
    pub reachable: bool,
    pub message: String,
    pub models: Vec<String>,
    pub model_details: Vec<EndpointModelDetail>,
    pub resolved_base_url: Option<String>,
}

/// Live-probe a provider credential. **Network, sends the secret to the provider** (and
/// reveals whether it is valid). No probe exists for some vars: the backend then answers
/// `ok: true, reachable: false` ("can't validate, don't block").
pub fn provider_validate_request(probe: &CredentialProbe) -> Request {
    let mut body = json!({
        "key": probe.key,
        "value": probe.value,
        "api_key": probe.api_key,
        "provider_setup": probe.provider_setup,
    });
    if !probe.profile.is_empty() {
        body["profile"] = json!(probe.profile);
    }
    req(Method::Post, "/api/providers/validate", Some(body))
}

/// Validate a credential (see [`provider_validate_request`]). Allow ~15 s: the backend
/// probes with a 10 s timeout and may try two URL variants.
pub async fn provider_validate(rest: &Rest, probe: &CredentialProbe) -> Result<CredentialValidation, ApiError> {
    crate::api::send_as(rest, &provider_validate_request(probe)).await
}

// ════════════════════════════════════════════════════════════════════════════════
// /api/providers/custom-endpoints
// ════════════════════════════════════════════════════════════════════════════════

/// One configured OpenAI-compatible endpoint. `source` is `providers` (v12 shape),
/// `custom_providers` (legacy row the migration left behind) or `direct-config` (a bare
/// `model.provider: custom` slot). `api_key_preview` is a mask — **not** the key.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct CustomEndpoint {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub model: String,
    pub models: Vec<String>,
    /// `""` = runtime auto-detect.
    pub api_mode: String,
    pub context_length: Option<i64>,
    pub discover_models: bool,
    pub has_api_key: bool,
    pub api_key_preview: Option<String>,
    pub is_current: bool,
    pub source: String,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct CustomEndpoints {
    pub endpoints: Vec<CustomEndpoint>,
    /// Present on the write replies only.
    pub ok: Option<bool>,
    /// The id the upsert created or updated.
    pub id: Option<String>,
}

/// List configured custom endpoints. Read-only, config only (no key material: previews are
/// masked).
pub fn custom_endpoints_request(profile: &str) -> Request {
    req(
        Method::Get,
        format!("/api/providers/custom-endpoints{}", profile_query(profile)),
        None,
    )
}

/// List custom endpoints (see [`custom_endpoints_request`]).
pub async fn custom_endpoints(rest: &Rest, profile: &str) -> Result<CustomEndpoints, ApiError> {
    crate::api::send_as(rest, &custom_endpoints_request(profile)).await
}

/// Create or update a custom endpoint (`CustomEndpointUpdate`). `id` empty creates one;
/// `api_key` is tri-state: `None` leaves the stored key alone, `Some("")` clears it,
/// `Some(key)` stores it in the profile's `.env`.
#[derive(Clone, Default, PartialEq)]
pub struct CustomEndpointUpdate {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    /// `""` | `chat_completions` | `codex_responses` | `anthropic_messages`; `None` leaves a
    /// hand-written `api_mode` alone.
    pub api_mode: Option<String>,
    pub context_length: Option<i64>,
    pub discover_models: bool,
    /// Also make it the main model.
    pub make_default: bool,
    pub models: Option<Vec<String>>,
}

impl CustomEndpointUpdate {
    pub fn new(name: &str, base_url: &str, model: &str) -> Self {
        Self {
            name: name.into(),
            base_url: base_url.into(),
            model: model.into(),
            discover_models: true,
            ..Self::default()
        }
    }

    fn to_body(&self) -> Value {
        let mut body = json!({
            "id": self.id,
            "name": self.name,
            "base_url": self.base_url,
            "model": self.model,
            "discover_models": self.discover_models,
            "make_default": self.make_default,
        });
        if let Some(key) = &self.api_key {
            body["api_key"] = json!(key);
        }
        if let Some(mode) = &self.api_mode {
            body["api_mode"] = json!(mode);
        }
        if let Some(ctx) = self.context_length {
            body["context_length"] = json!(ctx);
        }
        if let Some(models) = &self.models {
            body["models"] = json!(models);
        }
        body
    }
}

/// Create/update a custom endpoint. **Writes `config.yaml`** (and the key to `.env`).
/// `make_default` also repoints the main model.
pub fn custom_endpoint_upsert_request(profile: &str, update: &CustomEndpointUpdate) -> Request {
    req(
        Method::Post,
        format!("/api/providers/custom-endpoints{}", profile_query(profile)),
        Some(update.to_body()),
    )
}

/// Create/update a custom endpoint (see [`custom_endpoint_upsert_request`]).
pub async fn custom_endpoint_upsert(
    rest: &Rest,
    profile: &str,
    update: &CustomEndpointUpdate,
) -> Result<CustomEndpoints, ApiError> {
    crate::api::send_as(rest, &custom_endpoint_upsert_request(profile, update)).await
}

/// What an activation set: the provider key and model now in the main slot.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct CustomEndpointActivated {
    pub ok: bool,
    pub provider: String,
    pub model: String,
}

/// Make a custom endpoint the main model. **Writes `config.yaml`**; 400 when the endpoint has
/// no model/base_url, 404 when the id is unknown.
pub fn custom_endpoint_activate_request(profile: &str, endpoint_id: &str) -> Request {
    req(
        Method::Post,
        format!(
            "/api/providers/custom-endpoints/{}/activate{}",
            seg(endpoint_id),
            profile_query(profile)
        ),
        None,
    )
}

/// Activate a custom endpoint (see [`custom_endpoint_activate_request`]).
pub async fn custom_endpoint_activate(
    rest: &Rest,
    profile: &str,
    endpoint_id: &str,
) -> Result<CustomEndpointActivated, ApiError> {
    crate::api::send_as(rest, &custom_endpoint_activate_request(profile, endpoint_id)).await
}

/// Delete a custom endpoint. **Destructive**: removes the config entry, its `.env` key and
/// detaches the main model if it pointed here.
pub fn custom_endpoint_delete_request(profile: &str, endpoint_id: &str) -> Request {
    req(
        Method::Delete,
        format!(
            "/api/providers/custom-endpoints/{}{}",
            seg(endpoint_id),
            profile_query(profile)
        ),
        None,
    )
}

/// Delete a custom endpoint (see [`custom_endpoint_delete_request`]).
pub async fn custom_endpoint_delete(
    rest: &Rest,
    profile: &str,
    endpoint_id: &str,
) -> Result<CustomEndpoints, ApiError> {
    crate::api::send_as(rest, &custom_endpoint_delete_request(profile, endpoint_id)).await
}

/// The endpoint probe's verdict: which models it advertises, which base URL actually
/// answered, and which transport route was checked.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct CustomEndpointValidation {
    pub ok: bool,
    pub reachable: bool,
    pub message: String,
    pub models: Vec<String>,
    pub model_details: Vec<EndpointModelDetail>,
    /// The api mode whose route was probed (`chat_completions` | `codex_responses` |
    /// `anthropic_messages`), when the check got that far.
    pub transport_checked: Option<String>,
    pub resolved_base_url: Option<String>,
}

/// Probe a custom endpoint before saving it: GETs `{base}/models`, then POSTs a 1-token
/// request to the transport route the runtime would use. **Network**: contacts the URL the
/// user typed, with the key they typed (the endpoint sees both).
pub fn custom_endpoint_validate_request(update: &CustomEndpointUpdate) -> Request {
    req(
        Method::Post,
        "/api/providers/custom-endpoints/validate",
        Some(update.to_body()),
    )
}

/// Probe a custom endpoint (see [`custom_endpoint_validate_request`]). Allow ~20 s: two URL
/// attempts at 8 s plus the transport probe.
pub async fn custom_endpoint_validate(
    rest: &Rest,
    update: &CustomEndpointUpdate,
) -> Result<CustomEndpointValidation, ApiError> {
    crate::api::send_as(rest, &custom_endpoint_validate_request(update)).await
}

// ════════════════════════════════════════════════════════════════════════════════
// /api/local-models — managed llama.cpp runtime
// ════════════════════════════════════════════════════════════════════════════════

/// A downloaded (staged) model on disk, from `/api/local-models/status`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct StagedModel {
    /// The staged `.gguf` stem — the id `/activate` and `/eject` take. Sent as `id`
    /// (`local_models.py::_staged_row` emits only `id`, `size_bytes`, `size_label`); load
    /// state lives in [`LocalModelsStatus::loaded_models`].
    pub id: String,
    pub size_bytes: Option<i64>,
    pub size_label: Option<String>,
}

/// Cheap runtime/config state. `tag` is the installed engine build, `configured_tag` the
/// pinned one, `update_available` true when the pin has moved past the installed build.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct LocalModelsStatus {
    pub enabled: bool,
    pub tag: String,
    pub configured_tag: String,
    pub update_available: bool,
    pub runtime_installed: bool,
    pub runtime_backend: Option<String>,
    pub server_running: bool,
    pub server_base_url: Option<String>,
    /// Set when the active main model is one of the managed llama.cpp models.
    pub active_model_id: Option<String>,
    /// Free-form per-model placement facts keyed by model id.
    pub loaded_models: Value,
    pub loading: Value,
    pub placement: Value,
    pub models: Vec<StagedModel>,
    pub models_dir: String,
}

/// `GET /api/local-models/status` — config + installed runtime + staged models + supervisor
/// state, no GPU probing. Read-only on disk (it may adopt a legacy models directory).
pub fn local_models_status_request() -> Request {
    req(Method::Get, "/api/local-models/status", None)
}

/// Read the runtime status (see [`local_models_status_request`]).
pub async fn local_models_status(rest: &Rest) -> Result<LocalModelsStatus, ApiError> {
    crate::api::send_as(rest, &local_models_status_request()).await
}

/// The machine's model budget. GPU fields are `null` on non-NVIDIA hosts (the pane hides
/// them); `uma` marks a unified-memory machine (Apple/Strix Halo class).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct LocalHardware {
    pub uma: bool,
    pub vram_total_bytes: i64,
    pub vram_usable_bytes: i64,
    pub ram_total_bytes: i64,
    pub ram_available_bytes: i64,
    pub vram_label: String,
    pub gpu_name: Option<String>,
    pub gpu_util_percent: Option<f64>,
    pub vram_used_bytes: Option<i64>,
}

/// `GET /api/local-models/hardware` — VRAM/RAM budget. Read-only; may run `nvidia-smi` on
/// the backend host (cached per poll window).
pub fn local_hardware_request() -> Request {
    req(Method::Get, "/api/local-models/hardware", None)
}

/// Read the hardware budget (see [`local_hardware_request`]).
pub async fn local_hardware(rest: &Rest) -> Result<LocalHardware, ApiError> {
    crate::api::send_as(rest, &local_hardware_request()).await
}

/// One catalog row, already priced for THIS machine. When nothing fits, only the smallest
/// build's size and a `fit_summary` are present (`fits: false`, no `model_id`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct CatalogModel {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub native_context: i64,
    pub native_context_label: String,
    pub recommended: bool,
    pub recommended_reason: Option<String>,
    pub downloaded: bool,
    pub downloaded_model_id: Option<String>,
    pub downloaded_quant: Option<String>,
    pub mtp: bool,
    pub vision: bool,
    /// True when the entry needs a newer engine than the installed one — gate download.
    pub needs_engine: bool,
    pub min_engine: Option<String>,
    pub fits: bool,
    /// The variant this machine should download; absent when nothing fits.
    pub model_id: Option<String>,
    pub quant: Option<String>,
    pub quant_validated: Option<bool>,
    pub size_bytes: Option<i64>,
    pub size_label: Option<String>,
    pub variant_count: Option<i64>,
    pub quant_reason: Option<String>,
    pub fit_summary: Option<String>,
    pub fit_detail: Option<String>,
    pub start_window: Option<i64>,
    pub start_window_label: Option<String>,
    pub spilled: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct LocalCatalog {
    pub models: Vec<CatalogModel>,
}

/// `GET /api/local-models/catalog` — every entry with its price for this machine. Read-only
/// for the request, BUT it also kicks a TTL-gated **background refresh of the catalog over
/// the network** (Hugging Face / the catalog URL); the response itself serves the in-memory
/// catalog. Builder-only in the live test for that reason.
pub fn local_catalog_request() -> Request {
    req(Method::Get, "/api/local-models/catalog", None)
}

/// Read the catalog (see [`local_catalog_request`]).
pub async fn local_catalog(rest: &Rest) -> Result<LocalCatalog, ApiError> {
    crate::api::send_as(rest, &local_catalog_request()).await
}

/// A runtime or model job. `status` is `running` | `paused` | `done` | `error`; `phase` is a
/// human-readable step (`starting`, `downloading`, `verifying`, `starting-server`, `done`…).
/// `percent` appears only once `total_bytes` is known; `bytes_per_sec`/`eta_seconds` only
/// while running.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct LocalJob {
    pub job_id: String,
    /// `runtime-install` | `model-download` | `quickstart` | `model-activate`.
    pub kind: String,
    pub target: String,
    /// Catalog id for a model job, `None` otherwise.
    pub model_id: Option<String>,
    pub status: String,
    pub phase: String,
    pub detail: String,
    pub total_bytes: Option<i64>,
    pub done_bytes: i64,
    /// Unix seconds.
    pub started_at: f64,
    pub error: Option<String>,
    pub percent: Option<i64>,
    pub bytes_per_sec: Option<f64>,
    pub eta_seconds: Option<i64>,
    pub pause_requested: bool,
    pub can_pause: bool,
    pub can_resume: bool,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct LocalJobs {
    pub jobs: Vec<LocalJob>,
}

/// `GET /api/local-models/jobs` — every recent job, active ones first (the pane rediscovers
/// in-flight work here after a remount). Read-only, in-memory.
pub fn local_jobs_request() -> Request {
    req(Method::Get, "/api/local-models/jobs", None)
}

/// List jobs (see [`local_jobs_request`]).
pub async fn local_jobs(rest: &Rest) -> Result<LocalJobs, ApiError> {
    crate::api::send_as(rest, &local_jobs_request()).await
}

/// `GET /api/local-models/jobs/{job_id}` — poll one job. Read-only, in-memory; 404 when the
/// backend has forgotten it (a restart drops all jobs).
pub fn local_job_request(job_id: &str) -> Request {
    req(Method::Get, format!("/api/local-models/jobs/{}", seg(job_id)), None)
}

/// Poll one job (see [`local_job_request`]).
pub async fn local_job(rest: &Rest, job_id: &str) -> Result<LocalJob, ApiError> {
    crate::api::send_as(rest, &local_job_request(job_id)).await
}

/// An accepted job: `job_id` is `None` when the work was already done (`already_downloaded`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct JobAccepted {
    pub job_id: Option<String>,
    pub model_id: Option<String>,
    /// `runtime/install` only: the engine build chosen (`cuda`, `vulkan`, …) and its tag.
    pub backend: Option<String>,
    pub tag: Option<String>,
    pub already_downloaded: Option<bool>,
    pub ok: Option<bool>,
}

/// Install (or re-pin) the llama.cpp engine. **Downloads and unpacks a binary, spawns
/// processes** and may restart the local server; `backend` `None` = auto-detect.
pub fn runtime_install_request(profile: &str, backend: Option<&str>) -> Request {
    req(
        Method::Post,
        format!("/api/local-models/runtime/install{}", profile_query(profile)),
        Some(json!({ "backend": backend })),
    )
}

/// Install the runtime (see [`runtime_install_request`]).
pub async fn runtime_install(rest: &Rest, profile: &str, backend: Option<&str>) -> Result<JobAccepted, ApiError> {
    crate::api::send_as(rest, &runtime_install_request(profile, backend)).await
}

/// Download a catalog model. **Downloads gigabytes over the network** from Hugging Face;
/// `model_id` is a family id (the machine's selected variant) or an exact variant id. 409
/// when the engine is too old or no variant fits.
pub fn local_download_request(model_id: &str) -> Request {
    req(
        Method::Post,
        "/api/local-models/download",
        Some(json!({ "model_id": model_id })),
    )
}

/// Start a model download (see [`local_download_request`]).
pub async fn local_download(rest: &Rest, model_id: &str) -> Result<JobAccepted, ApiError> {
    crate::api::send_as(rest, &local_download_request(model_id)).await
}

/// `POST /api/local-models/download/pause` — pause a job's download phase. `paused: false`
/// means the job is not in a pausable phase (already finishing) — not an error.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct PauseReply {
    pub ok: bool,
    pub paused: bool,
}

/// Pause a download (see [`PauseReply`]). Reversible; keeps the job's partial bytes.
pub fn local_download_pause_request(job_id: &str) -> Request {
    req(
        Method::Post,
        "/api/local-models/download/pause",
        Some(json!({ "job_id": job_id })),
    )
}

/// Pause a download job.
pub async fn local_download_pause(rest: &Rest, job_id: &str) -> Result<PauseReply, ApiError> {
    crate::api::send_as(rest, &local_download_pause_request(job_id)).await
}

/// `POST /api/local-models/download/resume` — restart the same job; verified files and
/// durable ranges are reused. `resumed: false` when the job cannot resume (restart it).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ResumeReply {
    pub ok: bool,
    pub resumed: bool,
}

/// Resume a paused download job (see [`ResumeReply`]). **Network** transfer resumes.
pub fn local_download_resume_request(job_id: &str) -> Request {
    req(
        Method::Post,
        "/api/local-models/download/resume",
        Some(json!({ "job_id": job_id })),
    )
}

/// Resume a download job.
pub async fn local_download_resume(rest: &Rest, job_id: &str) -> Result<ResumeReply, ApiError> {
    crate::api::send_as(rest, &local_download_resume_request(job_id)).await
}

/// `DELETE /api/local-models/models/{model_id}` — delete a downloaded model. **Destructive**:
/// unlinks every split part and its assets from the models dir (a sideloaded file's hardlink
/// only; the original stays). 404 when nothing is on disk.
pub fn local_model_delete_request(model_id: &str) -> Request {
    req(
        Method::Delete,
        format!("/api/local-models/models/{}", seg(model_id)),
        None,
    )
}

/// Delete a downloaded model.
pub async fn local_model_delete(rest: &Rest, model_id: &str) -> Result<Value, ApiError> {
    crate::api::send(rest, &local_model_delete_request(model_id)).await
}

/// A quickstart's preflight: what the job will have to do, priced before it starts.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct QuickstartReply {
    pub job_id: String,
    pub model_id: String,
    pub display_name: String,
    pub needs_runtime: bool,
    pub needs_download: bool,
    pub download_bytes: i64,
}

/// One-click local setup: install the runtime if missing, download this machine's build of
/// the recommended model (or `model_id`), start the server and make it the default.
/// **Downloads a binary and gigabytes of model weights, spawns the server, writes
/// `config.yaml`**; 409 when a setup is already running or nothing fits.
pub fn quickstart_request(profile: &str, model_id: Option<&str>) -> Request {
    req(
        Method::Post,
        format!("/api/local-models/quickstart{}", profile_query(profile)),
        Some(json!({ "model_id": model_id })),
    )
}

/// Run the one-click local setup (see [`quickstart_request`]).
pub async fn quickstart(rest: &Rest, profile: &str, model_id: Option<&str>) -> Result<QuickstartReply, ApiError> {
    crate::api::send_as(rest, &quickstart_request(profile, model_id)).await
}

/// `POST /api/local-models/server` — turn the local engine on or off. `Off` IS durable (the
/// user said off, so boots stay off) and frees ALL GPU memory for every model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalServerAction {
    Start,
    Stop,
}

impl LocalServerAction {
    pub fn as_str(self) -> &'static str {
        match self {
            LocalServerAction::Start => "start",
            LocalServerAction::Stop => "stop",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ServerActionReply {
    pub ok: bool,
    pub action: String,
}

/// Start/stop the local server. **Spawns or kills a process and frees GPU memory**; `Stop`
/// also persists `runtime.enabled: false`. `Start` can take a while (model-less boot is fast;
/// a 502 means the server failed to come up).
pub fn local_server_request(action: LocalServerAction) -> Request {
    req(
        Method::Post,
        "/api/local-models/server",
        Some(json!({ "action": action.as_str() })),
    )
}

/// Start/stop the local server (see [`local_server_request`]).
pub async fn local_server(rest: &Rest, action: LocalServerAction) -> Result<ServerActionReply, ApiError> {
    crate::api::send_as(rest, &local_server_request(action)).await
}

/// `POST /api/local-models/eject` — unload one model from GPU memory now. The next message
/// reloads it; nothing else changes.
pub fn local_eject_request(model_id: &str) -> Request {
    req(
        Method::Post,
        "/api/local-models/eject",
        Some(json!({ "model_id": model_id })),
    )
}

/// Eject a model. **Frees GPU memory**: the next inference pays the reload. 409 when no
/// server is running.
pub async fn local_eject(rest: &Rest, model_id: &str) -> Result<Value, ApiError> {
    crate::api::send(rest, &local_eject_request(model_id)).await
}

/// `POST /api/local-models/activate` — make a downloaded model the default for new chats.
/// **Writes `config.yaml`**, enables the runtime and may start the server (no model load:
/// residency v2 loads on first inference). 404 unless the model is staged.
pub fn local_activate_request(profile: &str, model_id: &str) -> Request {
    req(
        Method::Post,
        format!("/api/local-models/activate{}", profile_query(profile)),
        Some(json!({ "model_id": model_id })),
    )
}

/// Activate a downloaded model (see [`local_activate_request`]).
pub async fn local_activate(rest: &Rest, profile: &str, model_id: &str) -> Result<JobAccepted, ApiError> {
    crate::api::send_as(rest, &local_activate_request(profile, model_id)).await
}

/// One Hugging Face search hit (`HFModelHit` in `local_runtime/hf_browse.py`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct HfModelHit {
    pub repo: String,
    pub downloads: i64,
    pub likes: i64,
    /// ISO date from HF (`lastModified`).
    pub updated: String,
    pub gated: bool,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct HfSearch {
    pub hits: Vec<HfModelHit>,
}

/// Search GGUF models on Hugging Face. **Network** to huggingface.co; 502 when unreachable.
/// The hit struct keeps only the stable fields (`#[serde(default)]` absorbs the rest).
pub fn local_search_request(q: &str, limit: u32) -> Request {
    let path = with_query("/api/local-models/search", &[("q", q), ("limit", &limit.to_string())]);
    req(Method::Get, path, None)
}

/// Search Hugging Face (see [`local_search_request`]).
pub async fn local_search(rest: &Rest, q: &str, limit: u32) -> Result<HfSearch, ApiError> {
    crate::api::send_as(rest, &local_search_request(q, limit)).await
}

/// One downloadable quant in an HF repo: a single GGUF or all parts of a split one, with a
/// rough pre-download fit verdict (`HFFileGroup` in `local_runtime/hf_browse.py`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct HfFileGroup {
    /// `Q4_K_M`, or the file stem when no quant was detected.
    pub label: String,
    /// The `.gguf` paths to pass to [`download_browsed_request`], in split order.
    pub paths: Vec<String>,
    pub total_bytes: i64,
    /// `fits-gpu` | `needs-ram` | `too-big` | `unknown`.
    pub fit: String,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct HfFiles {
    pub files: Vec<HfFileGroup>,
}

/// List the servable GGUFs in one HF repo. **Network** to huggingface.co; 502 on failure.
pub fn local_search_files_request(repo: &str) -> Request {
    req(
        Method::Get,
        with_query("/api/local-models/search/files", &[("repo", repo)]),
        None,
    )
}

/// List a repo's GGUFs (see [`local_search_files_request`]).
pub async fn local_search_files(rest: &Rest, repo: &str) -> Result<HfFiles, ApiError> {
    crate::api::send_as(rest, &local_search_files_request(repo)).await
}

/// Download arbitrary GGUFs from an HF repo into the managed models dir. **Network + disk**:
/// downloads the listed files; the result stages like a catalog model (`unverified` when the
/// catalog has no entry). 422 when no `.gguf` path is given.
pub fn download_browsed_request(repo: &str, paths: &[String]) -> Request {
    req(
        Method::Post,
        "/api/local-models/download-browsed",
        Some(json!({ "repo": repo, "paths": paths })),
    )
}

/// Download browsed files (see [`download_browsed_request`]).
pub async fn download_browsed(rest: &Rest, repo: &str, paths: &[String]) -> Result<JobAccepted, ApiError> {
    crate::api::send_as(rest, &download_browsed_request(repo, paths)).await
}

/// Register a `.gguf` already on the machine. **Writes to the models dir** (hardlink, then
/// symlink, then a full copy as fallbacks); the user's original file is never moved.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SideloadReply {
    pub ok: bool,
    pub model_id: String,
    pub already_present: Option<bool>,
}

/// Sideload a local GGUF (see [`SideloadReply`]). 422 when the path is not a `.gguf` file.
pub fn sideload_request(path: &str) -> Request {
    req(
        Method::Post,
        "/api/local-models/sideload",
        Some(json!({ "path": path })),
    )
}

/// Sideload a local model file.
pub async fn sideload(rest: &Rest, path: &str) -> Result<SideloadReply, ApiError> {
    crate::api::send_as(rest, &sideload_request(path)).await
}

// ════════════════════════════════════════════════════════════════════════════════
// RPC: model.disconnect / model.save_key
// ════════════════════════════════════════════════════════════════════════════════

/// `model.disconnect` result. `disconnected: false` never arrives — a disconnect that
/// removed nothing is error 4005 instead.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ModelDisconnect {
    pub slug: String,
    pub name: String,
    pub disconnected: bool,
}

/// Remove every credential (env keys AND OAuth/pool state) for a provider.
/// **Destructive**: the provider disappears from the picker until it is re-authenticated.
/// `session_id` binds the write to the session's profile (`""` = the gateway's launch
/// profile). Error 4005 when nothing was stored, 5035 when the env file refuses the write.
pub fn model_disconnect_call(slug: &str, session_id: &str) -> Call {
    let mut params = json!({ "slug": slug });
    if !session_id.is_empty() {
        params["session_id"] = json!(session_id);
    }
    call_of("model.disconnect", params)
}

/// Disconnect a provider (see [`model_disconnect_call`]).
pub async fn model_disconnect(client: &Client, slug: &str, session_id: &str) -> Result<ModelDisconnect, ApiError> {
    call_as(client, &model_disconnect_call(slug, session_id)).await
}

/// `model.save_key` result: the refreshed provider row (`model.options` shape, with
/// `authenticated: true` forced on).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ModelSaveKey {
    pub provider: ModelOptionProvider,
}

/// Save an API key for a provider. **Writes a secret** to the profile's `.env` and rotates
/// any stale `config.yaml` mirror of the old key. Refused on a managed install (4006), for an
/// unknown slug (4002), for a non-`api_key` auth type (4003). A same-key re-save is a no-op
/// apart from the refreshed row.
pub fn model_save_key_call(slug: &str, api_key: &str, session_id: &str) -> Call {
    let mut params = json!({ "slug": slug, "api_key": api_key });
    if !session_id.is_empty() {
        params["session_id"] = json!(session_id);
    }
    call_of("model.save_key", params)
}

/// Save a provider key (see [`model_save_key_call`]).
pub async fn model_save_key(
    client: &Client,
    slug: &str,
    api_key: &str,
    session_id: &str,
) -> Result<ModelSaveKey, ApiError> {
    call_as(client, &model_save_key_call(slug, api_key, session_id)).await
}

// ── Redacted `Debug` for credential-bearing types ───────────────────────────────
// These hold API keys or token tails; `{:?}` (logs, panics, test output) shows only whether
// a secret is present and its length.

fn secret_len(s: &str) -> String {
    if s.is_empty() {
        "<empty>".into()
    } else {
        format!("<redacted, {} chars>", s.chars().count())
    }
}

impl std::fmt::Debug for ModelAssignment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelAssignment")
            .field("scope", &self.scope)
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("task", &self.task)
            .field("reasoning_effort", &self.reasoning_effort)
            .field("base_url", &self.base_url)
            .field("api_key", &secret_len(&self.api_key))
            .field("confirm_expensive_model", &self.confirm_expensive_model)
            .field("profile", &self.profile)
            .finish()
    }
}

impl std::fmt::Debug for CredentialProbe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialProbe")
            .field("key", &self.key)
            .field("value", &secret_len(&self.value))
            .field("api_key", &secret_len(&self.api_key))
            .field("profile", &self.profile)
            .field("provider_setup", &self.provider_setup)
            .finish()
    }
}

impl std::fmt::Debug for CustomEndpointUpdate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CustomEndpointUpdate")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("api_key", &self.api_key.as_deref().map(secret_len))
            .field("api_mode", &self.api_mode)
            .field("context_length", &self.context_length)
            .field("discover_models", &self.discover_models)
            .field("make_default", &self.make_default)
            .field("models", &self.models)
            .finish()
    }
}

impl std::fmt::Debug for OAuthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthStatus")
            .field("logged_in", &self.logged_in)
            .field("source", &self.source)
            .field("source_label", &self.source_label)
            .field("token_preview", &self.token_preview.as_deref().map(secret_len))
            .field("expires_at", &self.expires_at)
            .field("has_refresh_token", &self.has_refresh_token)
            .field("last_refresh", &self.last_refresh)
            .field("free_tier", &self.free_tier)
            .field("account_tier", &self.account_tier)
            .field("error", &self.error)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn model_info_builder_and_decode() {
        let r = model_info_request("");
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (Method::Get, "/api/model/info", None)
        );
        assert_eq!(model_info_request("swe").path, "/api/model/info?profile=swe");

        let info: ModelInfo = decode(json!({
            "model": "glm-5", "provider": "cc", "auto_context_length": 200000,
            "config_context_length": 128000, "effective_context_length": 128000,
            "capabilities": {
                "supports_tools": true, "supports_vision": false, "supports_reasoning": true,
                "context_window": 200000, "max_output_tokens": 8192, "model_family": "glm"
            }
        }))
        .unwrap();
        assert_eq!(info.model, "glm-5");
        assert_eq!(info.auto_context_length, 200_000);
        assert_eq!(info.effective_context_length, 128_000);
        assert_eq!(info.capabilities.model_family.as_deref(), Some("glm"));
        assert_eq!(info.capabilities.supports_vision, Some(false));

        // No model configured: the backend answers the empty shape.
        let empty: ModelInfo = decode(json!({
            "model": "", "provider": "", "auto_context_length": 0, "config_context_length": 0,
            "effective_context_length": 0, "capabilities": {}
        }))
        .unwrap();
        assert_eq!(empty, ModelInfo::default());
        assert!(empty.capabilities.supports_tools.is_none());
        // Lenient against an older backend.
        assert_eq!(decode::<ModelInfo>(json!({})).unwrap(), ModelInfo::default());
    }

    #[test]
    fn model_options_builders() {
        let r = model_options_request("", false, false, true);
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (Method::Get, "/api/model/options?explicit_only=1", None)
        );
        assert_eq!(
            model_options_request("swe", true, true, false).path,
            "/api/model/options?refresh=1&include_unconfigured=1&profile=swe"
        );

        let c = model_options_call("", false, false, true, "");
        assert_eq!(
            (c.method, c.params),
            (
                "model.options",
                json!({ "explicit_only": true, "include_unconfigured": false, "refresh": false })
            )
        );
        let c = model_options_call("swe", true, true, false, "sess-1");
        assert_eq!(c.params["profile"], json!("swe"));
        assert_eq!(c.params["session_id"], json!("sess-1"));
        assert_eq!(c.params["refresh"], json!(true));
    }

    #[test]
    fn model_options_decodes_with_shared_protocol_type() {
        let o: ModelOptionsResult = decode(json!({
            "provider": "cc", "model": "glm-5",
            "providers": [
                { "slug": "cc", "name": "Command Code", "models": ["glm-5", {"id": "glm-5-fast"}],
                  "is_current": true, "authenticated": true, "is_user_defined": false,
                  "aliases": ["custom:cc"], "featured_models": ["glm-5"], "total_models": 2 },
                { "slug": "nous", "name": "Nous", "models": [], "warning": "no key" }
            ]
        }))
        .unwrap();
        assert_eq!(o.provider, "cc");
        assert_eq!(o.providers[0].model_ids(), vec!["glm-5", "glm-5-fast"]);
        assert_eq!(o.providers[0].catalog().featured, vec!["glm-5"]);
        assert_eq!(o.providers[1].total_models, None);
        assert_eq!(decode::<ModelOptionsResult>(json!({})).unwrap().providers.len(), 0);
    }

    #[test]
    fn model_set_builder_and_decode() {
        let a = ModelAssignment::main("cc", "glm-5");
        let r = model_set_request("", &a);
        assert_eq!(
            (r.method, r.path.as_str(), r.body.clone()),
            (
                Method::Post,
                "/api/model/set",
                Some(json!({
                    "scope": "main", "provider": "cc", "model": "glm-5", "task": "",
                    "base_url": "", "api_key": "", "confirm_expensive_model": false
                }))
            )
        );
        assert!(
            r.body.unwrap().get("reasoning_effort").is_none(),
            "unset effort must be omitted, not null"
        );

        let a = ModelAssignment::auxiliary("vision", "local", "qwen-vl")
            .with_base_url("http://127.0.0.1:8080/v1")
            .with_reasoning_effort(Some("high"))
            .confirm_expensive();
        let r = model_set_request("swe", &a);
        assert_eq!(r.path, "/api/model/set?profile=swe");
        let body = r.body.unwrap();
        assert_eq!(body["scope"], json!("auxiliary"));
        assert_eq!(body["task"], json!("vision"));
        assert_eq!(body["reasoning_effort"], json!("high"));
        assert_eq!(body["confirm_expensive_model"], json!(true));

        // Explicit null clears the override (the backend distinguishes the two by key presence).
        let omitted = model_set_request("", &ModelAssignment::auxiliary("vision", "auto", ""));
        assert!(omitted.body.unwrap().get("reasoning_effort").is_none());
        let r = model_set_request(
            "",
            &ModelAssignment::auxiliary("vision", "auto", "").with_reasoning_effort(None),
        );
        assert_eq!(r.body.unwrap()["reasoning_effort"], Value::Null);

        let asked: ModelAssignmentReply = decode(json!({
            "ok": false, "scope": "main", "provider": "anthropic", "model": "opus-5.5",
            "confirm_required": true, "confirm_message": "This model is expensive ($15/M in)."
        }))
        .unwrap();
        assert!(!asked.ok);
        assert!(asked.confirm_required);
        assert_eq!(
            asked.confirm_message.as_deref(),
            Some("This model is expensive ($15/M in).")
        );

        let saved: ModelAssignmentReply = decode(json!({
            "ok": true, "scope": "auxiliary", "tasks": ["vision"], "provider": "local", "model": "qwen-vl"
        }))
        .unwrap();
        assert!(saved.ok);
        assert_eq!(saved.tasks, ["vision"]);
        let reset_check: ModelAssignmentReply =
            decode(json!({"ok": true, "scope": "auxiliary", "reset": true})).unwrap();
        assert!(reset_check.reset && reset_check.tasks.is_empty());
    }

    #[test]
    fn credential_types_redact_debug() {
        let mut a = ModelAssignment::main("custom", "m");
        a.api_key = "fake-SECRET-123".into();
        let mut p = CredentialProbe::new("OPENAI_API_KEY", "fake-SECRET-123");
        p.api_key = "fake-SECRET-123".into();
        let e = CustomEndpointUpdate {
            api_key: Some("fake-SECRET-123".into()),
            ..Default::default()
        };
        let o = OAuthStatus {
            token_preview: Some("…SECRET-123".into()),
            ..Default::default()
        };
        for shown in [format!("{a:?}"), format!("{p:?}"), format!("{e:?}"), format!("{o:?}")] {
            assert!(!shown.contains("SECRET"), "leaked: {shown}");
            assert!(shown.contains("redacted"), "{shown}");
        }
        assert_eq!(
            decode::<ModelAssignmentReply>(json!({})).unwrap(),
            ModelAssignmentReply::default()
        );
    }

    #[test]
    fn auxiliary_builder_and_decode() {
        let r = auxiliary_models_request("");
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (Method::Get, "/api/model/auxiliary", None)
        );
        assert_eq!(auxiliary_models_request("swe").path, "/api/model/auxiliary?profile=swe");

        let a: AuxiliaryModels = decode(json!({
            "tasks": [
                { "task": "title", "provider": "auto", "model": "", "base_url": "",
                  "reasoning_effort": null, "local_endpoint": false },
                { "task": "vision", "provider": "local", "model": "qwen-vl",
                  "base_url": "http://127.0.0.1:8080/v1", "local_endpoint": true,
                  "label": "Screenshot reader", "hint": "Reads images", "plugin": "acme",
                  "inherit_from": "title",
                  "effective": { "provider": "auto", "model": "glm-5", "base_url": "" } }
            ],
            "main": { "provider": "cc", "model": "glm-5" }
        }))
        .unwrap();
        assert_eq!(a.tasks.len(), 2);
        assert_eq!(a.tasks[0].provider, "auto");
        assert!(a.tasks[0].label.is_none());
        assert!(a.tasks[1].local_endpoint);
        assert_eq!(a.tasks[1].inherit_from.as_deref(), Some("title"));
        let eff = a.tasks[1].effective.as_ref().unwrap();
        assert_eq!(eff.model, "glm-5");
        assert_eq!(a.main.model, "glm-5");
        assert_eq!(
            decode::<AuxiliaryModels>(json!({})).unwrap(),
            AuxiliaryModels::default()
        );
    }

    #[test]
    fn moa_builders_and_decode() {
        let r = moa_request("");
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (Method::Get, "/api/model/moa", None)
        );
        assert_eq!(moa_request("swe").path, "/api/model/moa?profile=swe");

        let cfg: MoaConfig = decode(json!({
            "default_preset": "default", "active_preset": "",
            "presets": {
                "default": {
                    "reference_models": [
                        { "provider": "cc", "model": "glm-5", "reasoning_effort": null, "enabled": true }
                    ],
                    "aggregator": { "provider": "cc", "model": "glm-5" },
                    "reference_temperature": 0.6, "aggregator_temperature": null,
                    "reference_timeout": null, "degraded_reference_policy": "loud",
                    "fanout": null, "enabled": true
                }
            },
            "reference_models": [{ "provider": "cc", "model": "glm-5" }],
            "aggregator": { "provider": "cc", "model": "glm-5" },
            "reference_temperature": 0.6, "aggregator_temperature": null,
            "reference_timeout": null, "degraded_reference_policy": "loud",
            "fanout": null, "enabled": true, "privacy_filter": "off"
        }))
        .unwrap();
        assert_eq!(cfg.default_preset, "default");
        assert_eq!(cfg.presets["default"].reference_models.len(), 1);
        assert_eq!(cfg.presets["default"].reference_temperature, Some(0.6));
        assert_eq!(cfg.privacy_filter, Some(json!("off")));

        // The PUT round-trips the same object.
        let r = moa_set_request("swe", &cfg);
        assert_eq!((r.method, r.path.as_str()), (Method::Put, "/api/model/moa?profile=swe"));
        let body = r.body.unwrap();
        assert_eq!(body["presets"]["default"]["aggregator"]["model"], json!("glm-5"));
        assert_eq!(body["reference_models"][0]["provider"], json!("cc"));
        assert_eq!(decode::<MoaConfig>(json!({})).unwrap(), MoaConfig::default());

        let saved: MoaConfig = decode(json!({ "ok": true, "default_preset": "default" })).unwrap();
        assert_eq!(saved.default_preset, "default");
    }

    #[test]
    fn recommended_default_builder_and_decode() {
        let r = recommended_default_request("", "nous");
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (Method::Get, "/api/model/recommended-default?provider=nous", None)
        );
        assert_eq!(
            recommended_default_request("swe", "openrouter").path,
            "/api/model/recommended-default?provider=openrouter&profile=swe"
        );

        let d: RecommendedDefault = decode(json!({
            "provider": "nous", "model": "hermes-4.5", "free_tier": true
        }))
        .unwrap();
        assert_eq!(d.model, "hermes-4.5");
        assert_eq!(d.free_tier, Some(true));

        let other: RecommendedDefault = decode(json!({
            "provider": "openrouter", "model": "", "free_tier": null
        }))
        .unwrap();
        assert!(other.model.is_empty());
        assert!(other.free_tier.is_none());
        assert_eq!(
            decode::<RecommendedDefault>(json!({})).unwrap(),
            RecommendedDefault::default()
        );
    }

    #[test]
    fn oauth_builders_and_decode() {
        assert_eq!(oauth_providers_request("swe").path, "/api/providers/oauth?profile=swe");
        assert_eq!(oauth_providers_request("").method, Method::Get);

        let start = oauth_start_request("", "openai-codex");
        assert_eq!(
            (start.method, start.path.as_str(), start.body),
            (Method::Post, "/api/providers/oauth/openai-codex/start", None)
        );
        assert_eq!(
            oauth_start_request("swe", "nous").path,
            "/api/providers/oauth/nous/start?profile=swe"
        );

        assert_eq!(
            oauth_poll_request("", "nous", "s1").path,
            "/api/providers/oauth/nous/poll/s1"
        );
        assert_eq!(
            oauth_poll_request("swe", "nous", "s1").path,
            "/api/providers/oauth/nous/poll/s1?profile=swe"
        );
        assert_eq!(
            oauth_cancel_request("swe", "s1").path,
            "/api/providers/oauth/sessions/s1?profile=swe"
        );
        assert_eq!(oauth_cancel_request("", "s1").method, Method::Delete);
        assert_eq!(
            oauth_disconnect_request("", "nous").method,
            Method::Delete,
            "disconnect is a DELETE on the provider"
        );
        assert_eq!(
            oauth_disconnect_request("swe", "nous").path,
            "/api/providers/oauth/nous?profile=swe"
        );
        let sub = oauth_submit_request("", "nous", "s1", "1234");
        assert_eq!(
            (sub.method, sub.path.as_str(), sub.body),
            (
                Method::Post,
                "/api/providers/oauth/nous/submit",
                Some(json!({ "session_id": "s1", "code": "1234" }))
            )
        );

        // A path segment from user data is encoded.
        assert_eq!(oauth_start_request("", "a/b").path, "/api/providers/oauth/a%2Fb/start");

        let list: OAuthProviders = decode(json!({
            "providers": [
                { "id": "nous", "name": "Nous Portal", "flow": "device_code",
                  "cli_command": "hermes auth add nous", "docs_url": "https://portal.nousresearch.com",
                  "disconnect_hint": null, "disconnect_command": null, "disconnectable": true,
                  "status": { "logged_in": true, "source": "nous_portal",
                              "source_label": "Nous Portal", "token_preview": "…ab12",
                              "expires_at": "2026-11-01T00:00:00+00:00",
                              "has_refresh_token": true, "free_tier": false,
                              "account_tier": "pro" } },
                { "id": "claude-code", "name": "Claude Code", "flow": "external",
                  "cli_command": "claude", "docs_url": "", "disconnect_hint": "Managed by that provider's CLI; remove it there.",
                  "disconnect_command": "rm -f ~/.claude/.credentials.json", "disconnectable": false,
                  "status": { "logged_in": false } }
            ]
        }))
        .unwrap();
        assert_eq!(list.providers.len(), 2);
        assert!(list.providers[0].status.logged_in);
        assert_eq!(
            list.providers[0].status.expires_at,
            Some(json!("2026-11-01T00:00:00+00:00"))
        );
        assert!(!list.providers[1].disconnectable);
        assert!(list.providers[1].status.token_preview.is_none());
        assert_eq!(decode::<OAuthProviders>(json!({})).unwrap().providers.len(), 0);

        let start: OAuthStart = decode(json!({
            "session_id": "01JABC", "flow": "device_code", "user_code": "ABCD-1234",
            "verification_url": "https://portal.nousresearch.com/device?code=ABCD-1234",
            "expires_in": 900, "poll_interval": 5
        }))
        .unwrap();
        assert_eq!(start.user_code, "ABCD-1234");
        assert_eq!(start.poll_interval, 5);
        assert_eq!(decode::<OAuthStart>(json!({})).unwrap(), OAuthStart::default());

        let poll: OAuthPoll = decode(json!({
            "session_id": "01JABC", "status": "approved", "error_message": null,
            "expires_at": 1761955200.0, "reason": "transfer_complete",
            "account_email": "user@example.com", "model": "hermes-4.5",
            "retryable": null, "retry_after": null
        }))
        .unwrap();
        assert_eq!(poll.status, "approved");
        assert_eq!(poll.expires_at, Some(1_761_955_200.0));
        assert!(poll.error_message.is_none());
        assert_eq!(decode::<OAuthPoll>(json!({})).unwrap(), OAuthPoll::default());

        let cancel: OAuthCancel = decode(json!({ "ok": false, "message": "session not found" })).unwrap();
        assert!(!cancel.ok);
        assert_eq!(cancel.message.as_deref(), Some("session not found"));
        let done: OAuthCancel = decode(json!({ "ok": true, "session_id": "01JABC" })).unwrap();
        assert_eq!(done.session_id.as_deref(), Some("01JABC"));

        let d: OAuthDisconnect = decode(json!({ "ok": true, "provider": "nous" })).unwrap();
        assert!(d.ok);
        assert_eq!(d.provider, "nous");
        assert_eq!(
            decode::<OAuthDisconnect>(json!({})).unwrap(),
            OAuthDisconnect::default()
        );
    }

    #[test]
    fn provider_validate_builder_and_decode() {
        let r = provider_validate_request(&CredentialProbe::new("OPENAI_API_KEY", "sk-test"));
        assert_eq!((r.method, r.path.as_str()), (Method::Post, "/api/providers/validate"));
        assert_eq!(
            r.body.clone().unwrap(),
            json!({
                "key": "OPENAI_API_KEY", "value": "sk-test", "api_key": "",
                "provider_setup": false
            })
        );
        assert!(r.body.unwrap().get("profile").is_none());

        let r = provider_validate_request(
            &CredentialProbe::new("OPENAI_BASE_URL", "http://127.0.0.1:8080/v1").with_api_key("sk-local"),
        );
        assert_eq!(r.body.unwrap()["api_key"], json!("sk-local"));

        let ok: CredentialValidation = decode(json!({
            "ok": true, "reachable": true, "message": "",
            "models": ["glm-5", "glm-5-fast"],
            "model_details": [{ "id": "glm-5-fast", "canonical_model": "glm-5", "reasoning_effort": "high" }],
            "resolved_base_url": "http://127.0.0.1:8080/v1"
        }))
        .unwrap();
        assert!(ok.ok);
        assert_eq!(ok.models.len(), 2);
        assert_eq!(ok.model_details[0].canonical_model.as_deref(), Some("glm-5"));

        // Bad key: ok false, reachable true. Unreachable: reachable false.
        let bad: CredentialValidation = decode(json!({
            "ok": false, "reachable": true, "message": "That API key was rejected."
        }))
        .unwrap();
        assert!(!bad.ok && bad.reachable);
        let offline: CredentialValidation = decode(json!({
            "ok": false, "reachable": false, "message": "Could not reach the provider to verify the key."
        }))
        .unwrap();
        assert!(!offline.reachable);
        assert_eq!(
            decode::<CredentialValidation>(json!({})).unwrap(),
            CredentialValidation::default()
        );
    }

    #[test]
    fn custom_endpoint_builders_and_decode() {
        assert_eq!(
            custom_endpoints_request("swe").path,
            "/api/providers/custom-endpoints?profile=swe"
        );

        let update = CustomEndpointUpdate::new("Local vLLM", "http://127.0.0.1:8000/v1", "glm-5");
        let r = custom_endpoint_upsert_request("", &update);
        assert_eq!(
            (r.method, r.path.as_str()),
            (Method::Post, "/api/providers/custom-endpoints")
        );
        let body = r.body.clone().unwrap();
        assert_eq!(
            body,
            json!({
                "id": "", "name": "Local vLLM", "base_url": "http://127.0.0.1:8000/v1",
                "model": "glm-5", "discover_models": true, "make_default": false
            })
        );
        assert!(
            body.get("api_key").is_none(),
            "an absent key leaves the stored one alone"
        );

        let mut clearing = update.clone();
        clearing.api_key = Some(String::new());
        clearing.api_mode = Some("chat_completions".into());
        clearing.context_length = Some(32_000);
        let body = custom_endpoint_upsert_request("", &clearing).body.unwrap();
        assert_eq!(body["api_key"], json!(""));
        assert_eq!(body["api_mode"], json!("chat_completions"));
        assert_eq!(body["context_length"], json!(32_000));

        assert_eq!(
            custom_endpoint_activate_request("", "local-vllm").path,
            "/api/providers/custom-endpoints/local-vllm/activate"
        );
        assert_eq!(
            custom_endpoint_delete_request("swe", "local vllm").path,
            "/api/providers/custom-endpoints/local%20vllm?profile=swe"
        );
        assert_eq!(custom_endpoint_delete_request("", "x").method, Method::Delete);
        assert_eq!(
            custom_endpoint_validate_request(&update).path,
            "/api/providers/custom-endpoints/validate"
        );

        let list: CustomEndpoints = decode(json!({
            "endpoints": [{
                "id": "local-vllm", "name": "Local vLLM", "base_url": "http://127.0.0.1:8000/v1",
                "model": "glm-5", "models": ["glm-5"], "api_mode": "chat_completions",
                "context_length": 32000, "discover_models": true, "has_api_key": true,
                "api_key_preview": "sk-…abcd", "is_current": true, "source": "providers"
            }]
        }))
        .unwrap();
        assert_eq!(list.endpoints[0].context_length, Some(32_000));
        assert!(list.endpoints[0].is_current);
        assert!(list.ok.is_none());
        assert_eq!(decode::<CustomEndpoints>(json!({})).unwrap().endpoints.len(), 0);

        let saved: CustomEndpoints = decode(json!({
            "endpoints": [], "ok": true, "id": "local-vllm"
        }))
        .unwrap();
        assert_eq!(saved.ok, Some(true));
        assert_eq!(saved.id.as_deref(), Some("local-vllm"));

        let act: CustomEndpointActivated = decode(json!({
            "ok": true, "provider": "local-vllm", "model": "glm-5"
        }))
        .unwrap();
        assert_eq!(act.provider, "local-vllm");
        assert_eq!(
            decode::<CustomEndpointActivated>(json!({})).unwrap(),
            CustomEndpointActivated::default()
        );

        let checked: CustomEndpointValidation = decode(json!({
            "ok": false, "reachable": true,
            "message": "http://127.0.0.1:8000/v1/models answered, but POST /chat/completions returned HTTP 404",
            "models": [], "model_details": [],
            "transport_checked": "chat_completions",
            "resolved_base_url": "http://127.0.0.1:8000/v1"
        }))
        .unwrap();
        assert!(!checked.ok && checked.reachable);
        assert_eq!(checked.transport_checked.as_deref(), Some("chat_completions"));
        assert_eq!(
            decode::<CustomEndpointValidation>(json!({})).unwrap(),
            CustomEndpointValidation::default()
        );
    }

    #[test]
    fn local_models_read_builders() {
        for (r, path) in [
            (local_models_status_request(), "/api/local-models/status"),
            (local_hardware_request(), "/api/local-models/hardware"),
            (local_catalog_request(), "/api/local-models/catalog"),
            (local_jobs_request(), "/api/local-models/jobs"),
        ] {
            assert_eq!((r.method, r.path.as_str(), r.body), (Method::Get, path, None));
        }
        assert_eq!(local_job_request("abc123").path, "/api/local-models/jobs/abc123");
        assert_eq!(
            local_search_request("qwen 3", 5).path,
            "/api/local-models/search?q=qwen%203&limit=5"
        );
        assert_eq!(
            local_search_files_request("unsloth/Qwen3-GGUF").path,
            "/api/local-models/search/files?repo=unsloth%2FQwen3-GGUF"
        );

        let status: LocalModelsStatus = decode(json!({
            "enabled": true, "tag": "b6123", "configured_tag": "b6123",
            "update_available": false, "runtime_installed": true, "runtime_backend": "vulkan",
            "server_running": false, "server_base_url": null, "active_model_id": null,
            "loaded_models": {}, "loading": {}, "placement": {},
            "models": [{ "id": "Qwen3-8B-Q4_K_M", "size_bytes": 5000000000i64,
                         "size_label": "4.7 GB" }],
            "models_dir": "/home/u/.hermes/models"
        }))
        .unwrap();
        assert!(status.runtime_installed);
        assert_eq!(status.models[0].id, "Qwen3-8B-Q4_K_M");
        assert_eq!(
            decode::<LocalModelsStatus>(json!({})).unwrap(),
            LocalModelsStatus::default()
        );

        let hw: LocalHardware = decode(json!({
            "uma": false, "vram_total_bytes": 8589934592i64, "vram_usable_bytes": 7516192768i64,
            "ram_total_bytes": 68719476736i64, "ram_available_bytes": 34359738368i64,
            "vram_label": "8.0 GB", "gpu_name": "NVIDIA GeForce RTX 3060",
            "gpu_util_percent": 3.0, "vram_used_bytes": 1073741824i64
        }))
        .unwrap();
        assert_eq!(hw.gpu_name.as_deref(), Some("NVIDIA GeForce RTX 3060"));
        assert!(decode::<LocalHardware>(json!({})).unwrap().gpu_name.is_none());

        let catalog: LocalCatalog = decode(json!({
            "models": [
                { "id": "qwen3-8b", "display_name": "Qwen3 8B", "description": "dense",
                  "native_context": 131072, "native_context_label": "128K",
                  "recommended": true, "recommended_reason": "Recommended build (Q4_K_M)",
                  "downloaded": true, "downloaded_model_id": "Qwen3-8B-Q4_K_M",
                  "downloaded_quant": "Q4_K_M", "mtp": false, "vision": false,
                  "needs_engine": false, "min_engine": null, "fits": true,
                  "model_id": "Qwen3-8B-Q4_K_M", "quant": "Q4_K_M", "quant_validated": true,
                  "size_bytes": 5000000000i64, "size_label": "4.7 GB", "variant_count": 3,
                  "quant_reason": "Recommended build (Q4_K_M)", "start_window": 65536,
                  "start_window_label": "64K", "spilled": false,
                  "fit_summary": "starts at 64K and grows toward 128K as you use it" },
                { "id": "huge-400b", "display_name": "Huge 400B", "description": "",
                  "native_context": 262144, "native_context_label": "256K",
                  "recommended": false, "downloaded": false, "mtp": false, "vision": false,
                  "needs_engine": false, "fits": false, "size_bytes": 200000000000i64,
                  "size_label": "186.3 GB",
                  "fit_summary": "Needs more memory than this machine has",
                  "fit_detail": "even the most compact build (Q2_K, 186.3 GB) exceeds GPU + system memory" }
            ]
        }))
        .unwrap();
        assert_eq!(catalog.models.len(), 2);
        assert!(catalog.models[0].recommended);
        assert_eq!(
            catalog.models[0].downloaded_model_id.as_deref(),
            Some("Qwen3-8B-Q4_K_M")
        );
        assert!(!catalog.models[1].fits);
        assert!(catalog.models[1].model_id.is_none());

        let jobs: LocalJobs = decode(json!({
            "jobs": [
                { "job_id": "j1", "kind": "model-download", "target": "Qwen3 8B (Q4_K_M)",
                  "model_id": "qwen3-8b", "status": "running", "phase": "downloading",
                  "detail": "Downloading model", "total_bytes": 5000000000i64, "done_bytes": 2500000000i64,
                  "started_at": 1761955000.0, "error": null, "percent": 50,
                  "bytes_per_sec": 12500000.0, "eta_seconds": 200,
                  "pause_requested": false, "can_pause": true, "can_resume": false },
                { "job_id": "j0", "kind": "runtime-install", "target": "llama.cpp b6123 (vulkan)",
                  "model_id": null, "status": "done", "phase": "done", "detail": "ready",
                  "total_bytes": null, "done_bytes": 0, "started_at": 1761950000.0, "error": null,
                  "pause_requested": false, "can_pause": false, "can_resume": false }
            ]
        }))
        .unwrap();
        assert_eq!(jobs.jobs[0].percent, Some(50));
        assert!(jobs.jobs[0].model_id.is_some());
        assert!(jobs.jobs[1].bytes_per_sec.is_none());
        assert_eq!(decode::<LocalJobs>(json!({})).unwrap().jobs.len(), 0);

        let hits: HfSearch = decode(json!({
            "hits": [{ "repo": "unsloth/Qwen3-8B-GGUF", "downloads": 1200, "likes": 30,
                       "updated": "2026-09-01T10:00:00Z", "gated": false }]
        }))
        .unwrap();
        assert_eq!(hits.hits[0].repo, "unsloth/Qwen3-8B-GGUF");
        assert_eq!(hits.hits[0].downloads, 1200);
        assert!(!hits.hits[0].gated);
        assert_eq!(decode::<HfSearch>(json!({})).unwrap().hits.len(), 0);

        let files: HfFiles = decode(json!({
            "files": [{ "label": "Q4_K_M", "paths": ["Qwen3-8B-Q4_K_M-00001-of-00002.gguf",
                       "Qwen3-8B-Q4_K_M-00002-of-00002.gguf"], "total_bytes": 5000000000i64,
                       "fit": "fits-gpu" }]
        }))
        .unwrap();
        assert_eq!(files.files[0].label, "Q4_K_M");
        assert_eq!(files.files[0].fit, "fits-gpu");
        assert_eq!(files.files[0].paths.len(), 2);
        assert_eq!(decode::<HfFiles>(json!({})).unwrap().files.len(), 0);
    }

    #[test]
    fn local_models_write_builders() {
        let r = runtime_install_request("", None);
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (
                Method::Post,
                "/api/local-models/runtime/install",
                Some(json!({ "backend": null }))
            )
        );
        assert_eq!(
            runtime_install_request("swe", Some("vulkan")).path,
            "/api/local-models/runtime/install?profile=swe"
        );
        assert_eq!(
            runtime_install_request("", Some("vulkan")).body.unwrap(),
            json!({ "backend": "vulkan" })
        );

        let r = local_download_request("qwen3-8b");
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (
                Method::Post,
                "/api/local-models/download",
                Some(json!({ "model_id": "qwen3-8b" }))
            )
        );
        assert_eq!(
            local_download_pause_request("j1").body.unwrap(),
            json!({ "job_id": "j1" })
        );
        assert_eq!(
            local_download_resume_request("j1").path,
            "/api/local-models/download/resume"
        );
        assert_eq!(
            (
                local_model_delete_request("a b").method,
                local_model_delete_request("a b").path.as_str()
            ),
            (Method::Delete, "/api/local-models/models/a%20b")
        );
        assert_eq!(quickstart_request("", None).body.unwrap(), json!({ "model_id": null }));
        assert_eq!(
            quickstart_request("swe", Some("qwen3-8b")).path,
            "/api/local-models/quickstart?profile=swe"
        );
        assert_eq!(
            local_server_request(LocalServerAction::Stop).body.unwrap(),
            json!({ "action": "stop" })
        );
        assert_eq!(
            local_server_request(LocalServerAction::Start).path,
            "/api/local-models/server"
        );
        assert_eq!(local_eject_request("m1").body.unwrap(), json!({ "model_id": "m1" }));
        assert_eq!(
            local_activate_request("swe", "m1").path,
            "/api/local-models/activate?profile=swe"
        );
        assert_eq!(
            download_browsed_request("unsloth/Qwen3-GGUF", &["a-Q4.gguf".to_owned()])
                .body
                .unwrap(),
            json!({ "repo": "unsloth/Qwen3-GGUF", "paths": ["a-Q4.gguf"] })
        );
        assert_eq!(
            sideload_request("/models/x.gguf").body.unwrap(),
            json!({ "path": "/models/x.gguf" })
        );

        let accepted: JobAccepted = decode(json!({
            "job_id": "j1", "model_id": "Qwen3-8B-Q4_K_M"
        }))
        .unwrap();
        assert_eq!(accepted.job_id.as_deref(), Some("j1"));
        let already: JobAccepted = decode(json!({
            "job_id": null, "already_downloaded": true, "model_id": "Qwen3-8B-Q4_K_M"
        }))
        .unwrap();
        assert!(already.job_id.is_none());
        assert_eq!(already.already_downloaded, Some(true));
        assert_eq!(decode::<JobAccepted>(json!({})).unwrap(), JobAccepted::default());

        let qs: QuickstartReply = decode(json!({
            "job_id": "j2", "model_id": "qwen3-8b", "display_name": "Qwen3 8B",
            "needs_runtime": true, "needs_download": true, "download_bytes": 5000000000i64
        }))
        .unwrap();
        assert!(qs.needs_runtime && qs.needs_download);
        assert_eq!(qs.download_bytes, 5_000_000_000);

        let pause: PauseReply = decode(json!({ "ok": true, "paused": false })).unwrap();
        assert!(pause.ok && !pause.paused);
        let resume: ResumeReply = decode(json!({ "ok": true, "resumed": true })).unwrap();
        assert!(resume.resumed);
        let act: ServerActionReply = decode(json!({ "ok": true, "action": "stop" })).unwrap();
        assert_eq!(act.action, "stop");
        let side: SideloadReply = decode(json!({
            "ok": true, "model_id": "x", "already_present": true
        }))
        .unwrap();
        assert_eq!(side.already_present, Some(true));
        assert_eq!(decode::<SideloadReply>(json!({})).unwrap(), SideloadReply::default());
    }

    #[test]
    fn rpc_builders_and_decode() {
        let c = model_disconnect_call("nous", "");
        assert_eq!((c.method, c.params), ("model.disconnect", json!({ "slug": "nous" })));
        let c = model_disconnect_call("nous", "sess-1");
        assert_eq!(c.params, json!({ "slug": "nous", "session_id": "sess-1" }));

        let c = model_save_key_call("cc", "sk-test", "");
        assert_eq!(
            (c.method, c.params),
            ("model.save_key", json!({ "slug": "cc", "api_key": "sk-test" }))
        );
        assert_eq!(
            model_save_key_call("cc", "k", "sess-1").params["session_id"],
            json!("sess-1")
        );

        let d: ModelDisconnect = decode(json!({
            "slug": "nous", "name": "Nous Portal", "disconnected": true
        }))
        .unwrap();
        assert!(d.disconnected);
        assert_eq!(d.name, "Nous Portal");
        assert_eq!(
            decode::<ModelDisconnect>(json!({})).unwrap(),
            ModelDisconnect::default()
        );

        let saved: ModelSaveKey = decode(json!({
            "provider": { "slug": "cc", "name": "Command Code", "models": ["glm-5"],
                          "is_current": false, "authenticated": true, "total_models": 1 }
        }))
        .unwrap();
        assert_eq!(saved.provider.slug, "cc");
        assert_eq!(saved.provider.authenticated, Some(true));
        assert_eq!(decode::<ModelSaveKey>(json!({})).unwrap().provider.slug, "");
    }
}
