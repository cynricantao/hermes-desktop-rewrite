//! Messaging gateways: platform cards (list, configure, test), Telegram and WhatsApp QR
//! onboarding, DM pairing (status, approve, revoke, clear) and webhook subscriptions
//! (list, create, enable, toggle, delete).
//!
//! Backed by the dashboard routes in `hermes_cli/web_routers/messaging.py` and
//! `hermes_cli/web_routers/ops.py`; the Electron desktop calls the same paths from
//! `apps/desktop/src/api/messaging.ts`. Follows the conventions in [`crate::api`]:
//! one pure builder + one async wrapper per endpoint, lenient response structs.
//!
//! Profile scope: every route here is profile-scoped. Reads and ordinary writes take
//! `?profile=` ([`profile_query`]); `DELETE /api/webhooks/{name}` uses the backend's
//! destructive guard and therefore always names the profile ([`explicit_profile_query`]).
//! `/api/pairing/approve` and `/api/pairing/revoke` read the profile off the request
//! *body*, not the query — the builders put it there.

use serde_json::{Map, Value, json};

use super::{ApiError, Method, Request, explicit_profile_query, profile_query, req, seg, send_as};
use crate::http::Rest;

/// The `{"ok": true}` answer of the routes that have nothing else to say.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct OkResponse {
    pub ok: bool,
}

// ── Platforms: list / configure / test ─────────────────────────

/// One env var a platform card exposes. Credentials arrive redacted
/// (`redacted_value`); only allowlists carry a `value` (they are not secrets).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct MessagingEnvVar {
    pub key: String,
    pub required: bool,
    pub is_set: bool,
    /// Display-only sentinel (`«redacted:…»`); `None` when unset.
    pub redacted_value: Option<String>,
    /// Saved plaintext, sent for allowlists only.
    pub value: Option<String>,
    pub description: String,
    pub prompt: String,
    pub help: String,
    pub url: Option<String>,
    pub is_password: bool,
    pub is_list: bool,
    pub advanced: bool,
}

/// WhatsApp-only card extras: bridge mode and which setup pieces are in place.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct WhatsAppSetup {
    /// `bot` / `self-chat`, or empty when unset.
    pub mode: String,
    pub allowed_users_set: bool,
    pub home_channel_set: bool,
}

/// One messaging platform card.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct MessagingPlatform {
    pub id: String,
    pub name: String,
    pub description: String,
    pub docs_url: String,
    pub enabled: bool,
    pub configured: bool,
    pub gateway_running: bool,
    /// `disabled` / `not_configured` / `pending_restart` / `connected` /
    /// `startup_failed` / `gateway_stopped` (live gateway verdict, if any).
    pub state: String,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub updated_at: Option<String>,
    /// `platforms.<id>.home_channel` as configured (free-form).
    pub home_channel: Option<Value>,
    pub env_vars: Vec<MessagingEnvVar>,
    /// Vendor callback URL, only while a multiplexer serves this profile.
    pub ingress_url: Option<String>,
    pub whatsapp_setup: Option<WhatsAppSetup>,
}

/// `GET /api/messaging/platforms` reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct MessagingPlatforms {
    pub env_path: String,
    pub gateway_start_command: String,
    pub platforms: Vec<MessagingPlatform>,
}

/// `PUT /api/messaging/platforms/{id}` reply. `hot_served` = a live multiplexer
/// rebuilt this profile's adapters from the new credentials (no restart needed).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct MessagingPlatformUpdateResult {
    pub ok: bool,
    pub platform: String,
    pub hot_served: Option<bool>,
}

/// `POST /api/messaging/platforms/{id}/test` reply. `auth_posture` is the
/// non-breaking extension (`open` / `allowlist` / `pairing` / `deny_all`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct MessagingPlatformTest {
    pub ok: bool,
    pub state: String,
    pub message: String,
    pub auth_posture: Option<String>,
}

/// `GET /api/messaging/platforms?profile=`: every catalog platform with its state.
pub fn messaging_platforms_request(profile: &str) -> Request {
    req(
        Method::Get,
        format!("/api/messaging/platforms{}", profile_query(profile)),
        None,
    )
}

/// List messaging platforms and their live state (read-only).
pub async fn messaging_platforms(rest: &Rest, profile: &str) -> Result<MessagingPlatforms, ApiError> {
    send_as(rest, &messaging_platforms_request(profile)).await
}

/// `PUT /api/messaging/platforms/{id}`: set credentials (`env`), `clear_env` keys
/// and/or the enable flag. Fields that are `None`/empty are left untouched.
/// **Writes credentials** (never secrets you did not intend to store).
pub fn messaging_platform_update_request(
    platform_id: &str,
    profile: &str,
    enabled: Option<bool>,
    env: &[(&str, &str)],
    clear_env: &[&str],
) -> Request {
    let mut body = Map::new();
    if let Some(enabled) = enabled {
        body.insert("enabled".into(), Value::Bool(enabled));
    }
    if !env.is_empty() {
        let pairs = env
            .iter()
            .map(|(k, v)| ((*k).to_owned(), Value::String((*v).to_owned())))
            .collect();
        body.insert("env".into(), Value::Object(pairs));
    }
    if !clear_env.is_empty() {
        body.insert("clear_env".into(), json!(clear_env));
    }
    // An explicit body profile beats the switcher's query param on the backend.
    if !profile.is_empty() {
        body.insert("profile".into(), Value::String(profile.to_owned()));
    }
    req(
        Method::Put,
        format!(
            "/api/messaging/platforms/{}{}",
            seg(platform_id),
            profile_query(profile)
        ),
        Some(Value::Object(body)),
    )
}

/// Save a platform's credentials/enable flag. **Writes credentials.**
pub async fn messaging_platform_update(
    rest: &Rest,
    platform_id: &str,
    profile: &str,
    enabled: Option<bool>,
    env: &[(&str, &str)],
    clear_env: &[&str],
) -> Result<MessagingPlatformUpdateResult, ApiError> {
    let r = messaging_platform_update_request(platform_id, profile, enabled, env, clear_env);
    send_as(rest, &r).await
}

/// `POST /api/messaging/platforms/{id}/test`: verdict on whether the platform is
/// actually reachable with the saved config. **Network** — it probes the live
/// gateway for the platform's connection state, so the UI confirms first.
pub fn messaging_platform_test_request(platform_id: &str, profile: &str) -> Request {
    req(
        Method::Post,
        format!(
            "/api/messaging/platforms/{}/test{}",
            seg(platform_id),
            profile_query(profile)
        ),
        None,
    )
}

/// Test a platform's live connection. **Network** (probes the gateway).
pub async fn messaging_platform_test(
    rest: &Rest,
    platform_id: &str,
    profile: &str,
) -> Result<MessagingPlatformTest, ApiError> {
    send_as(rest, &messaging_platform_test_request(platform_id, profile)).await
}

// ── Telegram QR onboarding ─────────────────────────────────────

/// `POST /api/messaging/telegram/onboarding/start` reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct TelegramOnboardingStart {
    pub pairing_id: String,
    pub suggested_username: String,
    pub deep_link: String,
    pub qr_payload: String,
    pub expires_at: String,
}

/// `GET /api/messaging/telegram/onboarding/{id}` reply: `waiting` until the user
/// finishes in Telegram, then `ready` with the minted bot's identity.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct TelegramOnboardingStatus {
    pub status: String,
    pub bot_username: Option<String>,
    pub owner_user_id: Option<String>,
    pub expires_at: String,
}

/// `POST /api/messaging/telegram/onboarding/{id}/apply` reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct TelegramOnboardingApply {
    pub ok: bool,
    pub platform: String,
    pub bot_username: Option<String>,
    pub needs_restart: bool,
    pub restart_started: Option<bool>,
    pub restart_action: Option<String>,
    pub restart_pid: Option<u64>,
    pub restart_error: Option<String>,
}

/// Start Telegram QR onboarding on the Nous pairing service. **Network** —
/// contacts `setup.hermes-agent.nousresearch.com` to mint a bot.
pub fn telegram_onboarding_start_request(profile: &str, bot_name: Option<&str>) -> Request {
    let body = match bot_name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(name) => json!({ "bot_name": name }),
        None => json!({}),
    };
    req(
        Method::Post,
        format!("/api/messaging/telegram/onboarding/start{}", profile_query(profile)),
        Some(body),
    )
}

/// Begin Telegram QR onboarding. **Network.**
pub async fn telegram_onboarding_start(
    rest: &Rest,
    profile: &str,
    bot_name: Option<&str>,
) -> Result<TelegramOnboardingStart, ApiError> {
    send_as(rest, &telegram_onboarding_start_request(profile, bot_name)).await
}

/// `GET /api/messaging/telegram/onboarding/{id}`: poll the QR flow's status.
/// **Network** — the backend asks the pairing service for the pairing's state.
pub fn telegram_onboarding_status_request(pairing_id: &str, profile: &str) -> Request {
    req(
        Method::Get,
        format!(
            "/api/messaging/telegram/onboarding/{}{}",
            seg(pairing_id),
            profile_query(profile)
        ),
        None,
    )
}

/// Poll a Telegram onboarding pairing. **Network.**
pub async fn telegram_onboarding_status(
    rest: &Rest,
    pairing_id: &str,
    profile: &str,
) -> Result<TelegramOnboardingStatus, ApiError> {
    send_as(rest, &telegram_onboarding_status_request(pairing_id, profile)).await
}

/// `POST /api/messaging/telegram/onboarding/{id}/apply`: write the minted bot
/// token + allowlist into the profile and restart the gateway best-effort.
/// **Network**, **writes credentials**, **spawns a gateway restart**.
pub fn telegram_onboarding_apply_request(pairing_id: &str, profile: &str, allowed_user_ids: &[&str]) -> Request {
    let mut body = Map::new();
    body.insert("allowed_user_ids".into(), json!(allowed_user_ids));
    if !profile.is_empty() {
        body.insert("profile".into(), Value::String(profile.to_owned()));
    }
    req(
        Method::Post,
        format!(
            "/api/messaging/telegram/onboarding/{}/apply{}",
            seg(pairing_id),
            profile_query(profile)
        ),
        Some(Value::Object(body)),
    )
}

/// Save a finished Telegram onboarding. **Network**, **writes credentials**,
/// **spawns a process** (best-effort gateway restart).
pub async fn telegram_onboarding_apply(
    rest: &Rest,
    pairing_id: &str,
    profile: &str,
    allowed_user_ids: &[&str],
) -> Result<TelegramOnboardingApply, ApiError> {
    let r = telegram_onboarding_apply_request(pairing_id, profile, allowed_user_ids);
    send_as(rest, &r).await
}

/// `DELETE /api/messaging/telegram/onboarding/{id}`: forget an in-flight pairing
/// (idempotent; nothing is saved and the pairing service is not told).
pub fn telegram_onboarding_cancel_request(pairing_id: &str, profile: &str) -> Request {
    req(
        Method::Delete,
        format!(
            "/api/messaging/telegram/onboarding/{}{}",
            seg(pairing_id),
            profile_query(profile)
        ),
        None,
    )
}

/// Cancel a Telegram onboarding pairing.
pub async fn telegram_onboarding_cancel(rest: &Rest, pairing_id: &str, profile: &str) -> Result<OkResponse, ApiError> {
    send_as(rest, &telegram_onboarding_cancel_request(pairing_id, profile)).await
}

// ── WhatsApp QR onboarding ─────────────────────────────────────

/// The WhatsApp onboarding payload, returned by start/status.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct WhatsAppOnboarding {
    pub pairing_id: String,
    /// `installing` / `starting` / `waiting` / `connected` / `error` / `expired` / `cancelled`.
    pub status: String,
    /// The QR payload to render while `waiting`.
    pub qr_payload: Option<String>,
    pub expires_at: String,
    pub mode: String,
    pub allowed_users: String,
    pub account_id: Option<String>,
    pub account_name: Option<String>,
    pub account_phone: Option<String>,
    pub error: Option<String>,
}

/// `POST /api/messaging/whatsapp/onboarding/{id}/apply` reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct WhatsAppOnboardingApply {
    pub ok: bool,
    pub platform: String,
    pub needs_restart: bool,
    pub restart_started: Option<bool>,
    pub restart_action: Option<String>,
    pub restart_pid: Option<u64>,
    pub restart_error: Option<String>,
}

/// Start WhatsApp QR onboarding. **Network** and **spawns a process** (installs
/// the bridge and runs the pairing child), so the UI confirms first.
pub fn whatsapp_onboarding_start_request(profile: &str, mode: Option<&str>, allowed_users: Option<&str>) -> Request {
    let mut body = Map::new();
    if let Some(mode) = mode.filter(|m| !m.is_empty()) {
        body.insert("mode".into(), Value::String(mode.to_owned()));
    }
    if let Some(users) = allowed_users.filter(|u| !u.is_empty()) {
        body.insert("allowed_users".into(), Value::String(users.to_owned()));
    }
    if !profile.is_empty() {
        body.insert("profile".into(), Value::String(profile.to_owned()));
    }
    req(
        Method::Post,
        format!("/api/messaging/whatsapp/onboarding/start{}", profile_query(profile)),
        Some(Value::Object(body)),
    )
}

/// Begin WhatsApp QR onboarding. **Network**, **spawns a process.**
pub async fn whatsapp_onboarding_start(
    rest: &Rest,
    profile: &str,
    mode: Option<&str>,
    allowed_users: Option<&str>,
) -> Result<WhatsAppOnboarding, ApiError> {
    let r = whatsapp_onboarding_start_request(profile, mode, allowed_users);
    send_as(rest, &r).await
}

/// `GET /api/messaging/whatsapp/onboarding/{id}`: poll the QR flow (local state;
/// 410 once expired). Not a network call.
pub fn whatsapp_onboarding_status_request(pairing_id: &str, profile: &str) -> Request {
    req(
        Method::Get,
        format!(
            "/api/messaging/whatsapp/onboarding/{}{}",
            seg(pairing_id),
            profile_query(profile)
        ),
        None,
    )
}

/// Poll a WhatsApp onboarding session.
pub async fn whatsapp_onboarding_status(
    rest: &Rest,
    pairing_id: &str,
    profile: &str,
) -> Result<WhatsAppOnboarding, ApiError> {
    send_as(rest, &whatsapp_onboarding_status_request(pairing_id, profile)).await
}

/// `POST /api/messaging/whatsapp/onboarding/{id}/apply`: save the linked account
/// into the profile and restart the gateway best-effort. **Writes credentials**,
/// **spawns a gateway restart.**
pub fn whatsapp_onboarding_apply_request(
    pairing_id: &str,
    profile: &str,
    mode: Option<&str>,
    allowed_users: Option<&str>,
) -> Request {
    let mut body = Map::new();
    if let Some(mode) = mode.filter(|m| !m.is_empty()) {
        body.insert("mode".into(), Value::String(mode.to_owned()));
    }
    if let Some(users) = allowed_users.filter(|u| !u.is_empty()) {
        body.insert("allowed_users".into(), Value::String(users.to_owned()));
    }
    if !profile.is_empty() {
        body.insert("profile".into(), Value::String(profile.to_owned()));
    }
    req(
        Method::Post,
        format!(
            "/api/messaging/whatsapp/onboarding/{}/apply{}",
            seg(pairing_id),
            profile_query(profile)
        ),
        Some(Value::Object(body)),
    )
}

/// Save a connected WhatsApp setup. **Writes credentials**, **spawns a process.**
pub async fn whatsapp_onboarding_apply(
    rest: &Rest,
    pairing_id: &str,
    profile: &str,
    mode: Option<&str>,
    allowed_users: Option<&str>,
) -> Result<WhatsAppOnboardingApply, ApiError> {
    let r = whatsapp_onboarding_apply_request(pairing_id, profile, mode, allowed_users);
    send_as(rest, &r).await
}

/// `DELETE /api/messaging/whatsapp/onboarding/{id}`: cancel a QR session and
/// terminate its pairing child (idempotent). **Terminates a process.**
pub fn whatsapp_onboarding_cancel_request(pairing_id: &str, profile: &str) -> Request {
    req(
        Method::Delete,
        format!(
            "/api/messaging/whatsapp/onboarding/{}{}",
            seg(pairing_id),
            profile_query(profile)
        ),
        None,
    )
}

/// Cancel a WhatsApp onboarding session. **Terminates a process.**
pub async fn whatsapp_onboarding_cancel(rest: &Rest, pairing_id: &str, profile: &str) -> Result<OkResponse, ApiError> {
    send_as(rest, &whatsapp_onboarding_cancel_request(pairing_id, profile)).await
}

// ── Pairing (who may DM the bot) ───────────────────────────────

/// A pending pairing request (carries `request_id` — the value to approve on,
/// never the DM'd code) or an already-approved user.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct PairingUser {
    pub platform: String,
    pub user_id: String,
    pub user_name: String,
    /// Pending rows only (empty on legacy pre-hash entries).
    pub request_id: Option<String>,
    /// Pending rows only.
    pub age_minutes: Option<i64>,
    /// Approved rows only (unix seconds).
    pub approved_at: Option<f64>,
}

/// `GET /api/pairing` reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct PairingState {
    pub pending: Vec<PairingUser>,
    pub approved: Vec<PairingUser>,
}

/// `POST /api/pairing/approve` reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct PairingApproveResult {
    pub ok: bool,
    pub user: Option<PairingUser>,
}

/// `POST /api/pairing/clear-pending` reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct PairingClearPendingResult {
    pub ok: bool,
    pub cleared: u32,
}

/// `GET /api/pairing?profile=`: pending requests and approved users (read-only).
pub fn pairing_request(profile: &str) -> Request {
    req(Method::Get, format!("/api/pairing{}", profile_query(profile)), None)
}

/// Read the pairing store (pending + approved).
pub async fn pairing(rest: &Rest, profile: &str) -> Result<PairingState, ApiError> {
    send_as(rest, &pairing_request(profile)).await
}

/// Body shared by both approve builders. The backend reads the profile off the
/// body (`body.profile or query`), and never off the query alone.
fn pairing_approve_body(platform: &str, field: &'static str, value: &str, profile: &str) -> Value {
    let mut body = Map::new();
    body.insert("platform".into(), Value::String(platform.to_owned()));
    body.insert(field.into(), Value::String(value.to_owned()));
    if !profile.is_empty() {
        body.insert("profile".into(), Value::String(profile.to_owned()));
    }
    Value::Object(body)
}

/// `POST /api/pairing/approve` on a pending row's `request_id` (what an admin
/// surface sends after listing pending requests).
pub fn pairing_approve_request(platform: &str, request_id: &str, profile: &str) -> Request {
    req(
        Method::Post,
        "/api/pairing/approve",
        Some(pairing_approve_body(platform, "request_id", request_id, profile)),
    )
}

/// Approve a pending pairing request by request id. **Grants access** to a user.
pub async fn pairing_approve(
    rest: &Rest,
    platform: &str,
    request_id: &str,
    profile: &str,
) -> Result<PairingApproveResult, ApiError> {
    send_as(rest, &pairing_approve_request(platform, request_id, profile)).await
}

/// `POST /api/pairing/approve` on the one-time `code` the user relays (case is
/// normalised to upper case by the backend).
pub fn pairing_approve_code_request(platform: &str, code: &str, profile: &str) -> Request {
    req(
        Method::Post,
        "/api/pairing/approve",
        Some(pairing_approve_body(platform, "code", code, profile)),
    )
}

/// Approve a pairing request by the user's one-time code. **Grants access**;
/// brute-forced codes can lock the platform out (429).
pub async fn pairing_approve_code(
    rest: &Rest,
    platform: &str,
    code: &str,
    profile: &str,
) -> Result<PairingApproveResult, ApiError> {
    send_as(rest, &pairing_approve_code_request(platform, code, profile)).await
}

/// `POST /api/pairing/revoke`: drop one user from the approved list. **Destructive**
/// (the user loses access).
pub fn pairing_revoke_request(platform: &str, user_id: &str, profile: &str) -> Request {
    let mut body = Map::new();
    body.insert("platform".into(), Value::String(platform.to_owned()));
    body.insert("user_id".into(), Value::String(user_id.to_owned()));
    if !profile.is_empty() {
        body.insert("profile".into(), Value::String(profile.to_owned()));
    }
    req(Method::Post, "/api/pairing/revoke", Some(Value::Object(body)))
}

/// Revoke a user's pairing grant. **Destructive.**
pub async fn pairing_revoke(rest: &Rest, platform: &str, user_id: &str, profile: &str) -> Result<OkResponse, ApiError> {
    send_as(rest, &pairing_revoke_request(platform, user_id, profile)).await
}

/// `POST /api/pairing/clear-pending`: drop every pending request for the profile.
/// **Destructive** (pending rows are lost, published codes stop working).
pub fn pairing_clear_pending_request(profile: &str) -> Request {
    req(
        Method::Post,
        format!("/api/pairing/clear-pending{}", profile_query(profile)),
        None,
    )
}

/// Clear all pending pairing requests. **Destructive.**
pub async fn pairing_clear_pending(rest: &Rest, profile: &str) -> Result<PairingClearPendingResult, ApiError> {
    send_as(rest, &pairing_clear_pending_request(profile)).await
}

// ── Webhooks (subscription CRUD) ───────────────────────────────

/// One webhook subscription. `secret` is never echoed here — only `secret_set`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct WebhookRoute {
    pub name: String,
    pub description: String,
    pub events: Vec<String>,
    /// `log` or a real target (`telegram`, `discord`, …).
    pub deliver: String,
    pub deliver_only: bool,
    pub prompt: String,
    pub script: String,
    pub skills: Vec<String>,
    pub created_at: Option<String>,
    /// Public receiver URL (`{base_url}/webhooks/{name}`).
    pub url: String,
    pub secret_set: bool,
    /// Default-enabled: only an explicit `false` turns a route off.
    pub enabled: bool,
}

/// `GET /api/webhooks` reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct WebhookState {
    pub enabled: bool,
    pub base_url: String,
    pub subscriptions: Vec<WebhookRoute>,
}

/// `POST /api/webhooks` reply: the route summary plus the HMAC secret, which is
/// surfaced exactly once, on create.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct WebhookCreated {
    /// One-time HMAC secret (`«redacted»` never appears here).
    pub secret: String,
    #[serde(flatten)]
    pub route: WebhookRoute,
}

/// `POST /api/webhooks/enable` reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct WebhookEnable {
    pub ok: bool,
    pub platform: String,
    pub enabled: bool,
    pub needs_restart: bool,
    pub restart_started: Option<bool>,
    pub restart_action: Option<String>,
    pub restart_pid: Option<u64>,
    pub restart_error: Option<String>,
}

/// `PUT /api/webhooks/{name}/enabled` reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct WebhookToggle {
    pub ok: bool,
    pub name: String,
    pub enabled: bool,
}

/// Body of `POST /api/webhooks`. Only `name` is required; an omitted `secret`
/// lets the backend generate one (returned once in [`WebhookCreated::secret`]).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct WebhookCreate {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    /// `log` (default) or a real delivery target.
    pub deliver: String,
    /// `true` delivers straight to `deliver`/`deliver_chat_id`, skipping the agent.
    pub deliver_only: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deliver_chat_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
}

impl WebhookCreate {
    /// A minimal subscription: log every event, nothing else. **Creates a route
    /// that accepts inbound HTTP** — its secret is returned once.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            deliver: "log".into(),
            ..Self::default()
        }
    }
}

/// `GET /api/webhooks?profile=`: the profile's subscription store (read-only).
pub fn webhooks_request(profile: &str) -> Request {
    req(Method::Get, format!("/api/webhooks{}", profile_query(profile)), None)
}

/// List webhook subscriptions and the receiver's base URL (read-only).
pub async fn webhooks(rest: &Rest, profile: &str) -> Result<WebhookState, ApiError> {
    send_as(rest, &webhooks_request(profile)).await
}

/// `POST /api/webhooks/enable`: turn the webhook platform on in the profile's
/// config and best-effort restart the gateway. **Spawns a process** — the restart
/// is why the UI asks first; the config write itself stays authoritative.
pub fn webhooks_enable_request(profile: &str) -> Request {
    req(
        Method::Post,
        format!("/api/webhooks/enable{}", profile_query(profile)),
        None,
    )
}

/// Enable the webhook platform. **Spawns a process** (gateway restart).
pub async fn webhooks_enable(rest: &Rest, profile: &str) -> Result<WebhookEnable, ApiError> {
    send_as(rest, &webhooks_enable_request(profile)).await
}

/// `POST /api/webhooks`: create (or replace) a subscription. Rejected with 400
/// while the webhook platform is disabled. **Stores a secret** (the route's HMAC
/// key, returned once in the reply).
pub fn webhook_create_request(profile: &str, body: &WebhookCreate) -> Request {
    req(
        Method::Post,
        format!("/api/webhooks{}", profile_query(profile)),
        Some(json!(body)),
    )
}

/// Create a webhook subscription; keep the one-time secret it returns.
pub async fn webhook_create(rest: &Rest, profile: &str, body: &WebhookCreate) -> Result<WebhookCreated, ApiError> {
    send_as(rest, &webhook_create_request(profile, body)).await
}

/// `PUT /api/webhooks/{name}/enabled`: enable/disable a route in place (it stays
/// on disk; a disabled route's events are rejected with 403). Hot-reloads.
pub fn webhook_set_enabled_request(name: &str, enabled: bool, profile: &str) -> Request {
    req(
        Method::Put,
        format!("/api/webhooks/{}/enabled{}", seg(name), profile_query(profile)),
        Some(json!({ "enabled": enabled })),
    )
}

/// Enable or disable one webhook subscription.
pub async fn webhook_set_enabled(
    rest: &Rest,
    name: &str,
    enabled: bool,
    profile: &str,
) -> Result<WebhookToggle, ApiError> {
    send_as(rest, &webhook_set_enabled_request(name, enabled, profile)).await
}

/// `DELETE /api/webhooks/{name}`: remove a subscription and its secret.
/// **Destructive** — uses the backend's destructive-profile guard, so the
/// profile is always named (`?profile=default` when the caller means default).
pub fn webhook_delete_request(name: &str, profile: &str) -> Request {
    req(
        Method::Delete,
        format!("/api/webhooks/{}{}", seg(name), explicit_profile_query(profile)),
        None,
    )
}

/// Delete a webhook subscription. **Destructive.**
pub async fn webhook_delete(rest: &Rest, name: &str, profile: &str) -> Result<OkResponse, ApiError> {
    send_as(rest, &webhook_delete_request(name, profile)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::decode;

    fn body_of(r: &Request) -> Value {
        r.body.clone().expect("body")
    }

    #[test]
    fn platforms_builders() {
        let r = messaging_platforms_request("");
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (Method::Get, "/api/messaging/platforms", None)
        );
        assert_eq!(
            messaging_platforms_request("swe").path,
            "/api/messaging/platforms?profile=swe"
        );

        // Full update: every field, profile in query and body, id encoded.
        let r = messaging_platform_update_request(
            "my platform",
            "swe",
            Some(true),
            &[("TELEGRAM_BOT_TOKEN", "123:abc")],
            &["TELEGRAM_ALLOWED_USERS"],
        );
        assert_eq!(r.method, Method::Put);
        assert_eq!(r.path, "/api/messaging/platforms/my%20platform?profile=swe");
        assert_eq!(
            body_of(&r),
            json!({
                "enabled": true,
                "env": {"TELEGRAM_BOT_TOKEN": "123:abc"},
                "clear_env": ["TELEGRAM_ALLOWED_USERS"],
                "profile": "swe",
            })
        );

        // Untouched fields are omitted entirely; the default profile is not named.
        let r = messaging_platform_update_request("telegram", "", None, &[], &[]);
        assert_eq!(r.path, "/api/messaging/platforms/telegram");
        assert_eq!(body_of(&r), json!({}));

        let r = messaging_platform_update_request("telegram", "", None, &[], &["X"]);
        assert_eq!(body_of(&r), json!({"clear_env": ["X"]}));

        let r = messaging_platform_test_request("telegram", "swe");
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (Method::Post, "/api/messaging/platforms/telegram/test?profile=swe", None)
        );
    }

    #[test]
    fn platforms_decode() {
        let v = json!({
            "env_path": "/home/x/.hermes/.env",
            "gateway_start_command": "hermes gateway start",
            "platforms": [{
                "id": "whatsapp",
                "name": "WhatsApp",
                "docs_url": "https://x",
                "enabled": true,
                "configured": false,
                "gateway_running": true,
                "state": "not_configured",
                "error_code": null,
                "error_message": null,
                "updated_at": null,
                "home_channel": {"type": "channel", "id": "c1"},
                "env_vars": [{
                    "key": "WHATSAPP_MODE", "required": true, "is_set": true,
                    "redacted_value": "«redacted:bo…t»", "value": "bot",
                    "description": "Mode", "prompt": "WhatsApp mode", "help": "",
                    "url": null, "is_password": false, "is_list": false, "advanced": true,
                }],
                "ingress_url": "https://example/p/ea/whatsapp",
                "whatsapp_setup": {"mode": "bot", "allowed_users_set": false, "home_channel_set": true},
                "extra_future_field": 1,
            }],
        });
        let p: MessagingPlatforms = decode(v).unwrap();
        assert_eq!(p.env_path, "/home/x/.hermes/.env");
        assert_eq!(p.platforms.len(), 1);
        let card = &p.platforms[0];
        assert_eq!(card.id, "whatsapp");
        assert!(card.enabled && !card.configured && card.gateway_running);
        assert_eq!(card.state, "not_configured");
        assert_eq!(card.error_code, None);
        assert_eq!(card.env_vars[0].value.as_deref(), Some("bot"));
        assert_eq!(card.env_vars[0].redacted_value.as_deref(), Some("«redacted:bo…t»"));
        assert_eq!(card.whatsapp_setup.as_ref().unwrap().mode, "bot");
        assert_eq!(card.ingress_url.as_deref(), Some("https://example/p/ea/whatsapp"));

        let empty: MessagingPlatforms = decode(json!({})).unwrap();
        assert_eq!(empty, MessagingPlatforms::default());
        // A null required field still fails loudly rather than silently emptying.
        assert!(decode::<MessagingPlatforms>(json!({"platforms": "nope"})).is_err());
    }

    #[test]
    fn platform_results_decode() {
        let u: MessagingPlatformUpdateResult =
            decode(json!({"ok": true, "platform": "telegram", "hot_served": false})).unwrap();
        assert_eq!(u.platform, "telegram");
        assert_eq!(u.hot_served, Some(false));

        let t: MessagingPlatformTest = decode(json!({
            "ok": true, "state": "connected", "message": "Telegram is connected.",
            "auth_posture": "deny_all"
        }))
        .unwrap();
        assert_eq!(t.auth_posture.as_deref(), Some("deny_all"));
        assert_eq!(t.state, "connected");

        let empty: MessagingPlatformTest = decode(json!({})).unwrap();
        assert_eq!(empty, MessagingPlatformTest::default());
    }

    #[test]
    fn telegram_builders() {
        let r = telegram_onboarding_start_request("", None);
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (
                Method::Post,
                "/api/messaging/telegram/onboarding/start",
                Some(json!({}))
            )
        );
        let r = telegram_onboarding_start_request("ea", Some("My Bot"));
        assert_eq!(r.path, "/api/messaging/telegram/onboarding/start?profile=ea");
        assert_eq!(r.body, Some(json!({"bot_name": "My Bot"})));
        // A blank name falls back to the backend default instead of blanking it.
        assert_eq!(telegram_onboarding_start_request("", Some("  ")).body, Some(json!({})));

        // Pairing ids go through seg(): a slash must not escape the path.
        let r = telegram_onboarding_status_request("a/b", "swe");
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (
                Method::Get,
                "/api/messaging/telegram/onboarding/a%2Fb?profile=swe",
                None
            )
        );

        let r = telegram_onboarding_apply_request("pid", "swe", &["12", "34"]);
        assert_eq!(
            (r.method, r.path.as_str()),
            (Method::Post, "/api/messaging/telegram/onboarding/pid/apply?profile=swe")
        );
        assert_eq!(
            r.body,
            Some(json!({"allowed_user_ids": ["12", "34"], "profile": "swe"}))
        );
        assert_eq!(
            telegram_onboarding_apply_request("pid", "", &[]).body,
            Some(json!({"allowed_user_ids": []}))
        );

        let r = telegram_onboarding_cancel_request("pid", "");
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (Method::Delete, "/api/messaging/telegram/onboarding/pid", None)
        );
    }

    #[test]
    fn telegram_decode() {
        let s: TelegramOnboardingStart = decode(json!({
            "pairing_id": "abc", "suggested_username": "hermes_agent_bot",
            "deep_link": "https://t.me/hermes_agent_bot?start=x", "qr_payload": "https://t.me/x",
            "expires_at": "2026-10-17T10:00:00Z"
        }))
        .unwrap();
        assert_eq!(s.pairing_id, "abc");
        assert_eq!(s.expires_at, "2026-10-17T10:00:00Z");

        let waiting: TelegramOnboardingStatus =
            decode(json!({"status": "waiting", "expires_at": "2026-10-17T10:00:00Z"})).unwrap();
        assert_eq!(waiting.status, "waiting");
        assert_eq!(waiting.bot_username, None);

        let ready: TelegramOnboardingStatus = decode(json!({
            "status": "ready", "bot_username": "hermes_agent_bot",
            "owner_user_id": "12345", "expires_at": "2026-10-17T10:00:00Z"
        }))
        .unwrap();
        assert_eq!(ready.owner_user_id.as_deref(), Some("12345"));

        let a: TelegramOnboardingApply = decode(json!({
            "ok": true, "platform": "telegram", "bot_username": "hermes_agent_bot",
            "needs_restart": false, "restart_started": true, "restart_action": "gateway-restart",
            "restart_pid": 4242
        }))
        .unwrap();
        assert_eq!(a.restart_pid, Some(4242));
        assert!(!a.needs_restart);

        let failed: TelegramOnboardingApply = decode(json!({
            "ok": true, "platform": "telegram", "bot_username": null,
            "needs_restart": true, "restart_started": false, "restart_error": "boom"
        }))
        .unwrap();
        assert_eq!(failed.restart_error.as_deref(), Some("boom"));
        assert_eq!(failed.restart_pid, None);

        let empty: TelegramOnboardingStatus = decode(json!({})).unwrap();
        assert_eq!(empty, TelegramOnboardingStatus::default());
    }

    #[test]
    fn whatsapp_builders() {
        let r = whatsapp_onboarding_start_request("", None, None);
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (
                Method::Post,
                "/api/messaging/whatsapp/onboarding/start",
                Some(json!({}))
            )
        );
        let r = whatsapp_onboarding_start_request("ea", Some("self-chat"), Some("+91 999"));
        assert_eq!(
            r.body,
            Some(json!({"mode": "self-chat", "allowed_users": "+91 999", "profile": "ea"}))
        );
        assert_eq!(r.path, "/api/messaging/whatsapp/onboarding/start?profile=ea");

        let r = whatsapp_onboarding_status_request("pid", "");
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (Method::Get, "/api/messaging/whatsapp/onboarding/pid", None)
        );

        let r = whatsapp_onboarding_apply_request("pid", "swe", None, Some("1,2"));
        assert_eq!(
            (r.method, r.path.as_str()),
            (Method::Post, "/api/messaging/whatsapp/onboarding/pid/apply?profile=swe")
        );
        assert_eq!(r.body, Some(json!({"allowed_users": "1,2", "profile": "swe"})));

        let r = whatsapp_onboarding_cancel_request("pid", "swe");
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (
                Method::Delete,
                "/api/messaging/whatsapp/onboarding/pid?profile=swe",
                None
            )
        );
    }

    #[test]
    fn whatsapp_decode() {
        let w: WhatsAppOnboarding = decode(json!({
            "pairing_id": "pid", "status": "waiting", "qr_payload": "2@abc",
            "expires_at": "2026-10-17T10:00:00Z", "mode": "bot", "allowed_users": "",
            "account_id": null, "account_name": null, "account_phone": null, "error": null
        }))
        .unwrap();
        assert_eq!(w.status, "waiting");
        assert_eq!(w.qr_payload.as_deref(), Some("2@abc"));
        assert_eq!(w.account_phone, None);

        let a: WhatsAppOnboardingApply = decode(json!({
            "ok": true, "platform": "whatsapp", "needs_restart": false,
            "restart_started": true, "restart_pid": 7
        }))
        .unwrap();
        assert_eq!(a.restart_pid, Some(7));

        let empty: WhatsAppOnboarding = decode(json!({})).unwrap();
        assert_eq!(empty, WhatsAppOnboarding::default());
    }

    #[test]
    fn pairing_builders() {
        let r = pairing_request("");
        assert_eq!((r.method, r.path.as_str(), r.body), (Method::Get, "/api/pairing", None));
        assert_eq!(pairing_request("swe").path, "/api/pairing?profile=swe");

        // Approve/revoke name the profile in the BODY (that is what the backend reads).
        let r = pairing_approve_request("telegram", "0123456789abcdef", "swe");
        assert_eq!(r.method, Method::Post);
        assert_eq!(r.path, "/api/pairing/approve");
        assert_eq!(
            r.body,
            Some(json!({
                "platform": "telegram", "request_id": "0123456789abcdef", "profile": "swe"
            }))
        );
        assert_eq!(
            pairing_approve_request("telegram", "req", "").body,
            Some(json!({"platform": "telegram", "request_id": "req"}))
        );

        let r = pairing_approve_code_request("slack", "ab12cd34", "");
        assert_eq!(r.body, Some(json!({"platform": "slack", "code": "ab12cd34"})));

        let r = pairing_revoke_request("telegram", "12345", "ea");
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (
                Method::Post,
                "/api/pairing/revoke",
                Some(json!({"platform": "telegram", "user_id": "12345", "profile": "ea"}))
            )
        );

        let r = pairing_clear_pending_request("swe");
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (Method::Post, "/api/pairing/clear-pending?profile=swe", None)
        );
    }

    #[test]
    fn pairing_decode() {
        let p: PairingState = decode(json!({
            "pending": [{
                "platform": "telegram", "request_id": "0123456789abcdef", "user_id": "42",
                "user_name": "Ada", "age_minutes": 3
            }],
            "approved": [{
                "platform": "telegram", "user_id": "7", "user_name": "Bob", "approved_at": 1750000000.5
            }]
        }))
        .unwrap();
        assert_eq!(p.pending[0].request_id.as_deref(), Some("0123456789abcdef"));
        assert_eq!(p.pending[0].age_minutes, Some(3));
        assert_eq!(p.approved[0].approved_at, Some(1_750_000_000.5));
        assert_eq!(p.approved[0].request_id, None);

        let ok: PairingApproveResult =
            decode(json!({"ok": true, "user": {"user_id": "42", "user_name": "Ada"}})).unwrap();
        assert_eq!(ok.user.unwrap().user_id, "42");

        let cleared: PairingClearPendingResult = decode(json!({"ok": true, "cleared": 2})).unwrap();
        assert_eq!(cleared.cleared, 2);

        let empty: PairingState = decode(json!({})).unwrap();
        assert_eq!(empty, PairingState::default());
        let ok_empty: OkResponse = decode(json!({})).unwrap();
        assert!(!ok_empty.ok);
    }

    #[test]
    fn webhook_builders() {
        let r = webhooks_request("");
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (Method::Get, "/api/webhooks", None)
        );
        assert_eq!(webhooks_request("ea").path, "/api/webhooks?profile=ea");

        let r = webhooks_enable_request("swe");
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (Method::Post, "/api/webhooks/enable?profile=swe", None)
        );

        let mut body = WebhookCreate::new("api test");
        body.description = Some("created by a test".into());
        body.events = vec!["deploy".into()];
        body.deliver_only = true;
        body.deliver = "telegram".into();
        body.deliver_chat_id = Some("42".into());
        let r = webhook_create_request("swe", &body);
        assert_eq!((r.method, r.path.as_str()), (Method::Post, "/api/webhooks?profile=swe"));
        assert_eq!(
            r.body,
            Some(json!({
                "name": "api test", "description": "created by a test", "events": ["deploy"],
                "deliver": "telegram", "deliver_only": true, "deliver_chat_id": "42"
            }))
        );
        // The minimal body: name + the log default, nothing else.
        assert_eq!(
            webhook_create_request("", &WebhookCreate::new("x")).body,
            Some(json!({"name": "x", "deliver": "log", "deliver_only": false}))
        );

        let r = webhook_set_enabled_request("my hook", false, "swe");
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (
                Method::Put,
                "/api/webhooks/my%20hook/enabled?profile=swe",
                Some(json!({"enabled": false}))
            )
        );

        // Delete always names the profile: the backend rejects an unnamed one on
        // a multi-profile host.
        let r = webhook_delete_request("my hook", "");
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (Method::Delete, "/api/webhooks/my%20hook?profile=default", None)
        );
        assert_eq!(
            webhook_delete_request("h", "coder").path,
            "/api/webhooks/h?profile=coder"
        );
    }

    #[test]
    fn webhook_decode() {
        let s: WebhookState = decode(json!({
            "enabled": true,
            "base_url": "http://localhost:8644",
            "subscriptions": [{
                "name": "alerts", "description": "d", "events": ["deploy"], "deliver": "telegram",
                "deliver_only": false, "prompt": "p", "script": "", "skills": ["s"],
                "created_at": "2026-10-17T09:00:00Z", "url": "http://localhost:8644/webhooks/alerts",
                "secret_set": true, "enabled": false
            }]
        }))
        .unwrap();
        assert!(s.enabled);
        assert_eq!(s.subscriptions[0].name, "alerts");
        assert!(!s.subscriptions[0].enabled);
        assert!(s.subscriptions[0].secret_set);

        // Create: route summary flattened beside the one-time secret.
        let c: WebhookCreated = decode(json!({
            "name": "alerts", "deliver": "log", "secret": "s3cr3t",
            "url": "http://localhost:8644/webhooks/alerts"
        }))
        .unwrap();
        assert_eq!(c.secret, "s3cr3t");
        assert_eq!(c.route.name, "alerts");
        assert_eq!(c.route.deliver, "log");
        assert_eq!(c.route.events, Vec::<String>::new());

        let e: WebhookEnable = decode(json!({
            "ok": true, "platform": "webhook", "enabled": true, "needs_restart": false,
            "restart_started": true, "restart_action": "gateway-restart", "restart_pid": 9
        }))
        .unwrap();
        assert_eq!(e.restart_pid, Some(9));
        let e2: WebhookEnable = decode(json!({
            "ok": true, "platform": "webhook", "enabled": true, "needs_restart": true,
            "restart_started": false, "restart_error": "no gateway"
        }))
        .unwrap();
        assert_eq!(e2.restart_error.as_deref(), Some("no gateway"));
        assert_eq!(e2.restart_pid, None);

        let t: WebhookToggle = decode(json!({"ok": true, "name": "alerts", "enabled": true})).unwrap();
        assert_eq!(t.name, "alerts");
        assert!(t.enabled);

        let empty: WebhookCreated = decode(json!({})).unwrap();
        assert_eq!(empty, WebhookCreated::default());
    }
}
