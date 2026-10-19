//! System: status, logs, ops (doctor, backup, security audit), env keys, updates, gateway restart, action status, analytics/insights/usage, setup status, i18n, machine facts, agents/subagents/delegation, spawn trees.
//!
//! Follows the conventions in [`crate::api`]. REST builders take `profile` (`""` = default) and
//! use [`profile_query`]; RPC builders send `"profile": null` for the default profile.

use std::collections::{BTreeMap, HashMap};
use std::fmt;

use serde_json::{Value, json};

use crate::api::{
    ApiError, Call, Method, Request, call_as, call_of, explicit_profile_query, profile_query, req, seg, send_as,
    with_query,
};
use crate::http::Rest;
use crate::rpc::Client;

/// `"profile"` for a JSON-RPC call: `null` means the backend's default profile.
fn profile_param(profile: &str) -> Value {
    if profile.is_empty() {
        Value::Null
    } else {
        Value::String(profile.to_owned())
    }
}

// ── Status ─────────────────────────────────────────────────────────────────────

/// One component row of `/api/status`' `components` map (`status`, optional `state`/`reason`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ComponentHealth {
    pub status: String,
    pub state: Option<String>,
    pub reason: Option<String>,
}

/// One live gateway in the profile topology (`ports` is a free-form list of platform bindings).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct GatewayRow {
    pub profile: String,
    pub ports: Value,
    pub served_profiles: Vec<String>,
}

/// `GET /api/status`: public liveness probe plus (on a loopback bind) host paths and PIDs.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SystemStatus {
    pub version: String,
    pub release_date: String,
    pub config_version: Option<u64>,
    pub latest_config_version: Option<u64>,
    pub can_update_hermes: bool,
    pub gateway_running: bool,
    /// `null` when no gateway has ever run.
    pub gateway_state: Option<String>,
    pub gateway_platforms: Value,
    pub gateway_exit_reason: Option<String>,
    pub gateway_updated_at: Option<f64>,
    pub gateway_heartbeat_stale_s: Option<f64>,
    pub gateway_shared_with: Option<Vec<String>>,
    pub active_agents: u64,
    pub gateway_busy: bool,
    pub gateway_drainable: bool,
    pub restart_drain_timeout: Option<f64>,
    pub active_sessions: u64,
    pub auth_required: bool,
    pub auth_providers: Vec<String>,
    pub auth_flows: Vec<String>,
    /// Tri-state from the backend: `valid`, `invalid` or `unknown`.
    pub nous_session_valid: String,
    pub install_id: Option<String>,
    pub shared_profile_warning: bool,
    pub components: HashMap<String, ComponentHealth>,
    pub overall: String,
    pub profiles: Vec<String>,
    pub parked_profiles: Vec<String>,
    pub gateway_mode: String,
    pub multiplex_standalone_reason: Option<String>,
    pub hermes_home: Option<String>,
    pub config_path: Option<String>,
    pub env_path: Option<String>,
    pub gateway_pid: Option<u32>,
    pub gateway_health_url: Option<String>,
    pub gateways: Vec<GatewayRow>,
}

pub fn system_status_request(profile: &str) -> Request {
    req(Method::Get, format!("/api/status{}", profile_query(profile)), None)
}

/// Machine-level liveness: version, gateway state, session count, auth gate, component rollup.
pub async fn system_status(rest: &Rest, profile: &str) -> Result<SystemStatus, ApiError> {
    send_as(rest, &system_status_request(profile)).await
}

// ── Logs ───────────────────────────────────────────────────────────────────────

/// `GET /api/logs`: the log file tail, newest last.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct Logs {
    pub file: String,
    pub lines: Vec<String>,
}

/// Builder for a log tail. `file` is one of `agent`, `errors`, `gateway`, `gui`, `desktop`,
/// `update`, `handoff`, `mcp`; `lines` is clamped by the backend (1–500, 2000 with `search`).
pub fn logs_request(
    file: &str,
    lines: u32,
    level: Option<&str>,
    component: Option<&str>,
    search: Option<&str>,
    profile: &str,
) -> Request {
    let lines_s = lines.to_string();
    let mut pairs: Vec<(&str, &str)> = vec![("file", file), ("lines", &lines_s)];
    if let Some(v) = level {
        pairs.push(("level", v));
    }
    if let Some(v) = component {
        pairs.push(("component", v));
    }
    if let Some(v) = search {
        pairs.push(("search", v));
    }
    pairs.push(("profile", profile));
    req(Method::Get, with_query("/api/logs", &pairs), None)
}

/// Read one log file's tail. Read-only; unknown `file`/`component` answer 400.
pub async fn logs(
    rest: &Rest,
    file: &str,
    lines: u32,
    level: Option<&str>,
    component: Option<&str>,
    search: Option<&str>,
    profile: &str,
) -> Result<Logs, ApiError> {
    send_as(rest, &logs_request(file, lines, level, component, search, profile)).await
}

// ── Env keys ───────────────────────────────────────────────────────────────────

/// One provider identity a credential can serve (rich form used by newer backends).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ProviderProfileRef {
    pub provider: String,
    pub provider_label: String,
    pub description: Option<String>,
    pub url: Option<String>,
    pub primary: Option<bool>,
}

/// One env-var row from `GET /api/env`; secrets are only ever the redacted preview.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct EnvVar {
    pub is_set: bool,
    pub redacted_value: Option<String>,
    pub description: String,
    pub url: Option<String>,
    pub category: String,
    pub is_password: bool,
    pub tools: Vec<String>,
    pub advanced: bool,
    pub channel_managed: bool,
    pub provider: String,
    pub provider_label: String,
    pub provider_profiles: Vec<ProviderProfileRef>,
    pub provider_primary: bool,
    pub custom: bool,
}

/// `GET /api/env` is a map keyed by variable name.
pub type EnvVars = HashMap<String, EnvVar>;

pub fn env_request(profile: &str) -> Request {
    req(Method::Get, format!("/api/env{}", profile_query(profile)), None)
}

/// Every known env key with redacted previews and provider grouping. **Reveals secrets** only as
/// the backend's redacted sentinel — never the real value.
pub async fn env_vars(rest: &Rest, profile: &str) -> Result<EnvVars, ApiError> {
    send_as(rest, &env_request(profile)).await
}

/// `POST /api/env/reveal` reply: the real secret. `Debug` deliberately hides it.
#[derive(Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct RevealedEnvVar {
    pub key: String,
    pub value: String,
}

impl fmt::Debug for RevealedEnvVar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RevealedEnvVar")
            .field("key", &self.key)
            .field("value", &"<redacted>")
            .finish()
    }
}

/// Builder for revealing one env var. **Reveals secrets** — confirm with the user first; the
/// backend rate-limits (5 per 30 s).
pub fn reveal_env_request(key: &str, profile: &str) -> Request {
    req(
        Method::Post,
        format!("/api/env/reveal{}", profile_query(profile)),
        Some(json!({ "key": key })),
    )
}

/// Fetch the real value of one env var. **Reveals secrets** — never log the reply.
pub async fn reveal_env(rest: &Rest, key: &str, profile: &str) -> Result<RevealedEnvVar, ApiError> {
    send_as(rest, &reveal_env_request(key, profile)).await
}

// ── Ops actions ────────────────────────────────────────────────────────────────

/// Reply of an action spawn (`doctor`, `security-audit`, `backup`, `gateway-restart`, update).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ActionStarted {
    pub ok: bool,
    pub pid: Option<u32>,
    pub name: String,
    pub action_id: Option<String>,
    pub already_running: Option<bool>,
    #[serde(rename = "HERMES_ACTION_ID")]
    pub hermes_action_id: Option<String>,
}

/// `POST /api/ops/backup` reply: the spawn result plus the archive path when one was generated.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct BackupStarted {
    #[serde(flatten)]
    pub action: ActionStarted,
    pub archive: Option<String>,
}

/// Builder for `hermes doctor` in the background. **Spawns a process**.
pub fn ops_doctor_request(profile: &str) -> Request {
    req(Method::Post, format!("/api/ops/doctor{}", profile_query(profile)), None)
}

/// Start `hermes doctor`; poll [`action_status`] with `"doctor"`. **Spawns a process**.
pub async fn ops_doctor(rest: &Rest, profile: &str) -> Result<ActionStarted, ApiError> {
    send_as(rest, &ops_doctor_request(profile)).await
}

/// Builder for `hermes security audit` in the background. **Spawns a process**.
pub fn ops_security_audit_request(profile: &str) -> Request {
    req(
        Method::Post,
        format!("/api/ops/security-audit{}", profile_query(profile)),
        None,
    )
}

/// Start `hermes security audit`. **Spawns a process**; poll [`action_status`] with
/// `"security-audit"`.
pub async fn ops_security_audit(rest: &Rest, profile: &str) -> Result<ActionStarted, ApiError> {
    send_as(rest, &ops_security_audit_request(profile)).await
}

/// Builder for `hermes backup`; `output` is the archive path (`None` = timestamped file in the
/// profile's dashboard backup dir). **Spawns a process**.
pub fn ops_backup_request(output: Option<&str>, profile: &str) -> Request {
    let body = match output {
        Some(o) => json!({ "output": o }),
        None => json!({}),
    };
    req(
        Method::Post,
        format!("/api/ops/backup{}", profile_query(profile)),
        Some(body),
    )
}

/// Start a backup. **Spawns a process**; poll [`action_status`] with `"backup"`.
pub async fn ops_backup(rest: &Rest, output: Option<&str>, profile: &str) -> Result<BackupStarted, ApiError> {
    send_as(rest, &ops_backup_request(output, profile)).await
}

/// `POST /api/ops/debug-share` reply: paste URLs plus what was redacted.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct DebugShare {
    pub ok: bool,
    pub urls: BTreeMap<String, String>,
    pub failures: Vec<String>,
    pub redacted: bool,
    pub auto_delete_seconds: Option<u64>,
}

/// Builder for uploading a redacted debug bundle. **Network** — uploads logs to paste services.
pub fn ops_debug_share_request(lines: u32, redact: bool, profile: &str) -> Request {
    req(
        Method::Post,
        format!("/api/ops/debug-share{}", profile_query(profile)),
        Some(json!({ "lines": lines, "redact": redact })),
    )
}

/// Upload the debug report and return copyable URLs. **Network** — leaves the machine.
pub async fn ops_debug_share(rest: &Rest, lines: u32, redact: bool, profile: &str) -> Result<DebugShare, ApiError> {
    send_as(rest, &ops_debug_share_request(lines, redact, profile)).await
}

// ── Action progress ────────────────────────────────────────────────────────────

/// `GET /api/actions/{name}/status`: action log tail, liveness and the durable update receipt.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ActionStatus {
    pub name: String,
    pub running: bool,
    pub exit_code: Option<i32>,
    pub pid: Option<u32>,
    pub lines: Vec<String>,
    pub action_id: Option<String>,
    /// Structured `hermes-update` receipt; free-form because its fields vary by action outcome.
    pub receipt: Option<Value>,
}

/// Builder for one action's progress; `lines` is clamped to 1–2000 by the backend.
pub fn action_status_request(name: &str, lines: u32) -> Request {
    let n = lines.to_string();
    let path = format!("/api/actions/{}/status", seg(name));
    req(Method::Get, with_query(&path, &[("lines", &n)]), None)
}

/// Tail an action's log. Unknown action names answer 404.
pub async fn action_status(rest: &Rest, name: &str, lines: u32) -> Result<ActionStatus, ApiError> {
    send_as(rest, &action_status_request(name, lines)).await
}

// ── Updates & gateway restart ──────────────────────────────────────────────────

/// One commit in an update check's "what's changed" list.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct UpdateCommit {
    pub sha: String,
    pub summary: String,
    pub author: String,
    pub at: i64,
}

/// `GET /api/hermes/update/check`: whether an update exists (no side effects).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct UpdateCheck {
    pub install_method: String,
    pub current_version: String,
    pub behind: Option<i64>,
    pub update_available: bool,
    pub can_apply: bool,
    pub update_command: String,
    pub message: Option<String>,
    pub rollback_to: Option<String>,
    pub channel: Option<String>,
    pub branch: Option<String>,
    pub source_version: Option<String>,
    pub commits: Vec<UpdateCommit>,
}

/// Builder for the update check. `force` busts the backend's 24 h cache. **Network**.
pub fn update_check_request(force: bool, profile: &str) -> Request {
    let force_s = if force { "true" } else { "" };
    req(
        Method::Get,
        with_query("/api/hermes/update/check", &[("force", force_s), ("profile", profile)]),
        None,
    )
}

/// Ask the backend whether a Hermes update is available. **Network** (GitHub API).
pub async fn update_check(rest: &Rest, force: bool, profile: &str) -> Result<UpdateCheck, ApiError> {
    send_as(rest, &update_check_request(force, profile)).await
}

/// Builder for applying an update in the background (`hermes update`).
/// **Spawns a process**; may restart the gateway.
pub fn update_apply_request() -> Request {
    req(Method::Post, "/api/hermes/update", None)
}

/// Start `hermes update`. **Spawns a process** and rewrites the install; poll
/// [`action_status`] with `"hermes-update"`.
pub async fn update_apply(rest: &Rest) -> Result<ActionStarted, ApiError> {
    send_as(rest, &update_apply_request()).await
}

/// Builder for restarting the gateway in the background. **Spawns a process**.
pub fn gateway_restart_request(profile: &str) -> Request {
    req(
        Method::Post,
        format!("/api/gateway/restart{}", profile_query(profile)),
        None,
    )
}

/// Restart the messaging gateway; the live WebSocket drops. **Spawns a process** (destructive to
/// in-flight turns).
pub async fn gateway_restart(rest: &Rest, profile: &str) -> Result<ActionStarted, ApiError> {
    send_as(rest, &gateway_restart_request(profile)).await
}

// ── Usage analytics ───────────────────────────────────────────────────────────

/// One calendar day of usage (`SUM(...)` columns can be null on an empty store).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct UsageDay {
    pub day: String,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub reasoning_tokens: Option<i64>,
    pub estimated_cost: Option<f64>,
    pub actual_cost: Option<f64>,
    pub sessions: Option<i64>,
    pub api_calls: Option<i64>,
}

/// One model's usage over the window.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct UsageModelRow {
    pub model: String,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub estimated_cost: Option<f64>,
    pub sessions: Option<i64>,
    pub api_calls: Option<i64>,
}

/// Window totals; the backend returns `null` when the store has no rows.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct UsageTotals {
    pub total_input: Option<i64>,
    pub total_output: Option<i64>,
    pub total_cache_read: Option<i64>,
    pub total_reasoning: Option<i64>,
    pub total_estimated_cost: Option<f64>,
    pub total_actual_cost: Option<f64>,
    pub total_sessions: Option<i64>,
    pub total_api_calls: Option<i64>,
}

/// Skill-usage rollup; shapes are free-form inside the insights engine.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct UsageSkills {
    pub summary: Value,
    pub top_skills: Vec<Value>,
}

/// One tool's call count.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct UsageTool {
    pub tool: String,
    pub count: i64,
    pub percentage: f64,
}

/// `GET /api/analytics/usage`: daily/model/task/skill/tool usage for `days` (1–365).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct UsageAnalytics {
    pub daily: Vec<UsageDay>,
    pub by_model: Vec<UsageModelRow>,
    pub by_task: Vec<Value>,
    pub totals: Option<UsageTotals>,
    pub period_days: u32,
    pub skills: UsageSkills,
    pub tools: Vec<UsageTool>,
}

/// Builder for the usage analytics window. `days` is clamped to 1–365 by the backend.
pub fn usage_analytics_request(days: u32, profile: &str) -> Request {
    let d = days.to_string();
    req(
        Method::Get,
        with_query("/api/analytics/usage", &[("days", &d), ("profile", profile)]),
        None,
    )
}

/// Usage analytics for the last `days`. Read-only SQL over the session store.
pub async fn usage_analytics(rest: &Rest, days: u32, profile: &str) -> Result<UsageAnalytics, ApiError> {
    send_as(rest, &usage_analytics_request(days, profile)).await
}

// ── Media ─────────────────────────────────────────────────────────────────────

/// `GET /api/media` / `/api/media/proxy`: a base64 data URL.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct MediaData {
    pub data_url: String,
}

/// Builder for reading a gateway-local image as a data URL. 415/403/404/413 on rejection.
pub fn media_request(path: &str) -> Request {
    req(Method::Get, with_query("/api/media", &[("path", path)]), None)
}

/// Read a local image inside the backend's media roots as a data URL. Read-only.
pub async fn media(rest: &Rest, path: &str) -> Result<MediaData, ApiError> {
    send_as(rest, &media_request(path)).await
}

/// Builder for proxying a remote image through the gateway. **Network** (allowlisted CDNs).
pub fn media_proxy_request(url: &str) -> Request {
    req(Method::Get, with_query("/api/media/proxy", &[("url", url)]), None)
}

/// Fetch a remote image the client cannot reach, as a data URL. **Network** — builder only until
/// a UI needs it.
pub async fn media_proxy(rest: &Rest, url: &str) -> Result<MediaData, ApiError> {
    send_as(rest, &media_proxy_request(url)).await
}

// ── Config summary ────────────────────────────────────────────────────────────

/// One display section of `config.show` (`rows` are pre-rendered label/value pairs).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ConfigSection {
    pub title: String,
    pub rows: Vec<Vec<String>>,
}

/// `config.show`: masked, display-ready config rows (no secrets).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ConfigShow {
    pub sections: Vec<ConfigSection>,
}

pub fn config_show_call(profile: &str) -> Call {
    call_of("config.show", json!({ "profile": profile_param(profile) }))
}

/// Read the masked config summary. Read-only; values are already redacted by the backend.
pub async fn config_show(client: &Client, profile: &str) -> Result<ConfigShow, ApiError> {
    call_as(client, &config_show_call(profile)).await
}

// ── Setup ─────────────────────────────────────────────────────────────────────

/// `setup.status`: loose provider check (is ANY provider auth state discoverable).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SetupStatus {
    pub provider_configured: Option<bool>,
    pub ready: Option<bool>,
    pub free_tier_account: Option<bool>,
    pub free_tier_route: Option<bool>,
    pub other_providers: Option<bool>,
    pub inference_provider: Option<String>,
    pub profile: Option<String>,
    pub ok: Option<bool>,
    pub error: Option<String>,
    pub error_code: Option<String>,
    pub retryable: Option<bool>,
    pub retry_after: Option<i64>,
}

/// `setup.runtime_check`: strict readiness through the same resolver session creation uses.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SetupRuntimeCheck {
    pub ok: bool,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub source: Option<String>,
    pub error: Option<String>,
    pub free_tier_route: Option<bool>,
    pub profile: Option<String>,
}

pub fn setup_status_call(profile: &str) -> Call {
    call_of("setup.status", json!({ "profile": profile_param(profile) }))
}

/// Loose onboarding check: does any provider credential exist? Read-only.
pub async fn setup_status(client: &Client, profile: &str) -> Result<SetupStatus, ApiError> {
    call_as(client, &setup_status_call(profile)).await
}

/// Builder for the strict runtime check; `provider` pins one provider instead of the fallback
/// chain.
pub fn setup_runtime_check_call(provider: Option<&str>, profile: &str) -> Call {
    call_of(
        "setup.runtime_check",
        json!({
            "profile": profile_param(profile),
            "provider": provider.map(Value::from).unwrap_or(Value::Null),
        }),
    )
}

/// Probe whether a session could start (strict resolver). Read-only, no model call.
pub async fn setup_runtime_check(
    client: &Client,
    provider: Option<&str>,
    profile: &str,
) -> Result<SetupRuntimeCheck, ApiError> {
    call_as(client, &setup_runtime_check_call(provider, profile)).await
}

// ── i18n ──────────────────────────────────────────────────────────────────────

/// One selectable language supplied by a layer.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct LanguageOption {
    pub id: String,
    pub endonym: String,
    pub rtl: bool,
    pub source: String,
}

/// `i18n.languages`: bundled ∪ user overlay ∪ plugin packs, English first.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct I18nLanguages {
    pub languages: Vec<LanguageOption>,
}

/// `i18n.catalog`: one language + surface's messages to merge over the bundled catalog.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct I18nCatalog {
    pub lang: String,
    pub surface: String,
    pub messages: BTreeMap<String, String>,
}

pub fn i18n_languages_call(profile: &str) -> Call {
    call_of("i18n.languages", json!({ "profile": profile_param(profile) }))
}

/// List every language a layer supplies. Read-only.
pub async fn i18n_languages(client: &Client, profile: &str) -> Result<I18nLanguages, ApiError> {
    call_as(client, &i18n_languages_call(profile)).await
}

/// Builder for one surface's translation pack (`surface`: `core`, `tui` or `desktop`).
pub fn i18n_catalog_call(lang: &str, surface: &str, profile: &str) -> Call {
    call_of(
        "i18n.catalog",
        json!({
            "profile": profile_param(profile),
            "lang": lang,
            "surface": surface,
        }),
    )
}

/// Fetch a language's messages for one surface. Read-only.
pub async fn i18n_catalog(client: &Client, lang: &str, surface: &str, profile: &str) -> Result<I18nCatalog, ApiError> {
    call_as(client, &i18n_catalog_call(lang, surface, profile)).await
}

// ── Machine & system ─────────────────────────────────────────────────────────

/// Host facts (`full_name` is stripped by the backend).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct MachineInfo {
    pub os_family: Option<String>,
    pub os_release: Option<String>,
    pub native_arch: Option<String>,
    pub cpu_model: Option<String>,
    pub ram_gb: Option<i64>,
    pub gpu_class: Option<String>,
    pub vendor: Option<String>,
    pub wsl: Option<bool>,
    pub container: Option<bool>,
}

/// `machine.facts`: the machine and OS account, for first-run onboarding.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct MachineFacts {
    pub machine: MachineInfo,
    pub machine_kind: String,
    pub has_nvidia_gpu: bool,
    pub nvidia_platform: Option<String>,
    pub locale: Option<String>,
}

/// `system.battery`: host battery for the status bar (`available: false` when unreadable).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct BatteryStatus {
    pub available: bool,
    pub percent: Option<i64>,
    pub plugged: Option<bool>,
    pub category: String,
}

pub fn machine_facts_call() -> Call {
    call_of("machine.facts", json!({}))
}

/// Read host facts (OS, CPU, RAM, GPU class). Read-only.
pub async fn machine_facts(client: &Client) -> Result<MachineFacts, ApiError> {
    call_as(client, &machine_facts_call()).await
}

pub fn system_battery_call(profile: &str) -> Call {
    call_of("system.battery", json!({ "profile": profile_param(profile) }))
}

/// Read the host battery. Read-only; always resolves (`available: false` without one).
pub async fn system_battery(client: &Client, profile: &str) -> Result<BatteryStatus, ApiError> {
    call_as(client, &system_battery_call(profile)).await
}

/// `gateway.capabilities`: what this build actually enforces.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct GatewayCapabilities {
    pub per_session_exclusive_submit: bool,
}

pub fn gateway_capabilities_call() -> Call {
    call_of("gateway.capabilities", json!({}))
}

/// Read the gateway's enforced capability flags. Read-only.
pub async fn gateway_capabilities(client: &Client) -> Result<GatewayCapabilities, ApiError> {
    call_as(client, &gateway_capabilities_call()).await
}

// ── Agents & verification ────────────────────────────────────────────────────

/// One background agent process in the registry-wide summary.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct AgentProcess {
    pub session_id: String,
    pub command: String,
    pub status: String,
    pub uptime: i64,
}

/// `agents.list`: every registered background process.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct AgentsList {
    pub processes: Vec<AgentProcess>,
}

/// Verification evidence for one cwd/session.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct VerificationInfo {
    pub status: String,
    /// Evidence row; free-form so extra columns from newer backends do not break decoding.
    pub evidence: Option<Value>,
    pub root: Option<String>,
    pub session_id: Option<String>,
    pub changed_paths: Option<Vec<String>>,
}

/// `verification.status`: best known evidence; never runs checks.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct VerificationStatus {
    pub verification: VerificationInfo,
}

pub fn agents_list_call(profile: &str) -> Call {
    call_of("agents.list", json!({ "profile": profile_param(profile) }))
}

/// List the registry's background agent processes. Read-only.
pub async fn agents_list(client: &Client, profile: &str) -> Result<AgentsList, ApiError> {
    call_as(client, &agents_list_call(profile)).await
}

/// Builder for verification evidence; all scoping fields are optional.
pub fn verification_status_call(
    session_id: Option<&str>,
    session_key: Option<&str>,
    cwd: Option<&str>,
    profile: &str,
) -> Call {
    let mut p = json!({ "profile": profile_param(profile) });
    if let Some(v) = session_id {
        p["session_id"] = json!(v);
    }
    if let Some(v) = session_key {
        p["session_key"] = json!(v);
    }
    if let Some(v) = cwd {
        p["cwd"] = json!(v);
    }
    call_of("verification.status", p)
}

/// Read verification evidence for a cwd/session. Read-only; never runs checks.
pub async fn verification_status(
    client: &Client,
    session_id: Option<&str>,
    session_key: Option<&str>,
    cwd: Option<&str>,
    profile: &str,
) -> Result<VerificationStatus, ApiError> {
    call_as(client, &verification_status_call(session_id, session_key, cwd, profile)).await
}

// ── Insights & usage bars ────────────────────────────────────────────────────

/// `insights.get`: session/message counts over the window.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct Insights {
    pub days: i64,
    pub sessions: i64,
    pub messages: i64,
}

/// One bar of the two-bar dollar usage view.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct UsageBar {
    pub kind: String,
    pub remaining_display: String,
    pub total_display: String,
    pub spent_display: String,
    pub pct_used: Option<i64>,
    pub fill_fraction: f64,
}

/// `usage.bars`: plan + top-up spend, shared by `/usage`, `/topup` and `/subscription`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct UsageBars {
    pub ok: Option<bool>,
    pub available: bool,
    pub status: Option<String>,
    pub plan_name: Option<String>,
    pub renews_at: Option<String>,
    pub renews_display: Option<String>,
    pub subscription_remaining_display: Option<String>,
    pub topup_remaining_display: Option<String>,
    pub total_spendable_display: Option<String>,
    pub has_topup: Option<bool>,
    pub plan_bar: Option<UsageBar>,
    pub topup_bar: Option<UsageBar>,
}

pub fn insights_get_call(days: Option<u32>, profile: &str) -> Call {
    let mut p = json!({ "profile": profile_param(profile) });
    if let Some(d) = days {
        p["days"] = json!(d);
    }
    call_of("insights.get", p)
}

/// Session/message counts over the last `days`. Read-only SQL.
pub async fn insights_get(client: &Client, days: Option<u32>, profile: &str) -> Result<Insights, ApiError> {
    call_as(client, &insights_get_call(days, profile)).await
}

pub fn usage_bars_call(profile: &str) -> Call {
    call_of("usage.bars", json!({ "profile": profile_param(profile) }))
}

/// Two-bar dollar usage view. **Network** — the backend fetches the Nous portal when logged in.
pub async fn usage_bars(client: &Client, profile: &str) -> Result<UsageBars, ApiError> {
    call_as(client, &usage_bars_call(profile)).await
}

// ── Delegation & subagents ───────────────────────────────────────────────────

/// One active subagent in the delegation tree.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ActiveSubagent {
    pub subagent_id: String,
    pub parent_id: Option<String>,
    pub depth: Option<i64>,
    pub goal: Option<String>,
    pub delegation_id: Option<String>,
    pub model: Option<String>,
    pub started_at: Option<f64>,
    pub status: Option<String>,
    pub tool_count: Option<i64>,
    pub owner_agent_session_id: Option<String>,
}

/// `delegation.status`: running tree plus the spawn pause flag and limits.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct DelegationStatus {
    pub active: Vec<ActiveSubagent>,
    pub paused: bool,
    pub max_spawn_depth: i64,
    pub max_concurrent_children: i64,
}

/// `delegation.pause` reply: the new global spawn state.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct DelegationPause {
    pub paused: bool,
}

/// Live child owned by one session.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SubagentSnapshot {
    pub subagent_id: String,
    pub parent_id: Option<String>,
    pub depth: Option<i64>,
    pub goal: Option<String>,
    pub delegation_id: Option<String>,
    pub model: Option<String>,
    pub started_at: Option<f64>,
    pub status: Option<String>,
    pub tool_count: Option<i64>,
    pub last_tool: Option<String>,
    pub accepting_steer: Option<bool>,
}

/// A delegation whose task(s) finished badly.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct FailedDelegation {
    pub delegation_id: String,
    pub task_index: i64,
    pub status: String,
    pub goal: String,
    pub error: Option<String>,
    pub dispatched_at: Option<f64>,
    pub completed_at: Option<f64>,
}

/// `subagent.list`: this session's live children plus failed delegations.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SubagentList {
    pub subagents: Vec<SubagentSnapshot>,
    pub delegations: Vec<FailedDelegation>,
}

/// `subagent.tail`: the last 16 KB of one child's transcript.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SubagentTail {
    pub subagent_id: String,
    pub available: bool,
    pub text: String,
    pub truncated: bool,
}

/// `subagent.steer`: whether the steering text was queued.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SubagentSteer {
    pub status: String,
    pub subagent_id: String,
    pub text: String,
}

/// `subagent.interrupt`: `found` is false when the child already finished.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SubagentInterrupt {
    pub found: bool,
    pub subagent_id: String,
}

pub fn delegation_status_call(profile: &str) -> Call {
    call_of("delegation.status", json!({ "profile": profile_param(profile) }))
}

/// Read the delegation tree and limits. Read-only.
pub async fn delegation_status(client: &Client, profile: &str) -> Result<DelegationStatus, ApiError> {
    call_as(client, &delegation_status_call(profile)).await
}

/// Builder for pausing (`paused: true`) or resuming new spawns globally.
pub fn delegation_pause_call(paused: bool, profile: &str) -> Call {
    call_of(
        "delegation.pause",
        json!({ "profile": profile_param(profile), "paused": paused }),
    )
}

/// Pause/resume new spawns (active children keep running). Reversible; not run live.
pub async fn delegation_pause(client: &Client, paused: bool, profile: &str) -> Result<DelegationPause, ApiError> {
    call_as(client, &delegation_pause_call(paused, profile)).await
}

pub fn subagent_list_call(session_id: &str, profile: &str) -> Call {
    call_of(
        "subagent.list",
        json!({ "profile": profile_param(profile), "session_id": session_id }),
    )
}

/// List this session's live children. 4001 when the session is unknown.
pub async fn subagent_list(client: &Client, session_id: &str, profile: &str) -> Result<SubagentList, ApiError> {
    call_as(client, &subagent_list_call(session_id, profile)).await
}

pub fn subagent_tail_call(session_id: &str, subagent_id: &str, profile: &str) -> Call {
    call_of(
        "subagent.tail",
        json!({
            "profile": profile_param(profile),
            "session_id": session_id,
            "subagent_id": subagent_id,
        }),
    )
}

/// Tail one owned child's transcript. 4001 when the session is unknown.
pub async fn subagent_tail(
    client: &Client,
    session_id: &str,
    subagent_id: &str,
    profile: &str,
) -> Result<SubagentTail, ApiError> {
    call_as(client, &subagent_tail_call(session_id, subagent_id, profile)).await
}

/// Builder for queueing steering text into a live child.
pub fn subagent_steer_call(session_id: &str, subagent_id: &str, text: &str, profile: &str) -> Call {
    call_of(
        "subagent.steer",
        json!({
            "profile": profile_param(profile),
            "session_id": session_id,
            "subagent_id": subagent_id,
            "text": text,
        }),
    )
}

/// Queue steering text into a live child. Write; affects a running agent — confirm first.
pub async fn subagent_steer(
    client: &Client,
    session_id: &str,
    subagent_id: &str,
    text: &str,
    profile: &str,
) -> Result<SubagentSteer, ApiError> {
    call_as(client, &subagent_steer_call(session_id, subagent_id, text, profile)).await
}

pub fn subagent_interrupt_call(session_id: &str, subagent_id: &str, profile: &str) -> Call {
    call_of(
        "subagent.interrupt",
        json!({
            "profile": profile_param(profile),
            "session_id": session_id,
            "subagent_id": subagent_id,
        }),
    )
}

/// Hard-interrupt one owned child. **Destructive** to that child's work — confirm first.
pub async fn subagent_interrupt(
    client: &Client,
    session_id: &str,
    subagent_id: &str,
    profile: &str,
) -> Result<SubagentInterrupt, ApiError> {
    call_as(client, &subagent_interrupt_call(session_id, subagent_id, profile)).await
}

// ── Spawn trees ──────────────────────────────────────────────────────────────

/// One saved spawn-tree snapshot in the list.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SpawnTreeEntry {
    pub path: String,
    pub session_id: Option<String>,
    pub started_at: Option<f64>,
    pub finished_at: Option<f64>,
    pub label: String,
    pub count: i64,
}

/// `spawn_tree.list`: saved snapshots, newest first.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SpawnTreeList {
    pub entries: Vec<SpawnTreeEntry>,
}

/// `spawn_tree.save`: where the snapshot landed.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SpawnTreeSaved {
    pub path: String,
    pub session_id: String,
}

/// `spawn_tree.load`: one snapshot's metadata and raw subagent records.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SpawnTreeSnapshot {
    pub session_id: Option<String>,
    pub started_at: Option<f64>,
    pub finished_at: Option<f64>,
    pub label: Option<String>,
    pub subagents: Vec<Value>,
}

/// Builder for persisting a finished delegation tree. `subagents` are the raw record objects the
/// gateway already streamed (free-form, keyed by `subagent_id`).
pub fn spawn_tree_save_call(
    subagents: Vec<Value>,
    session_id: Option<&str>,
    started_at: Option<f64>,
    finished_at: Option<f64>,
    label: Option<&str>,
    profile: &str,
) -> Call {
    let mut p = json!({
        "profile": profile_param(profile),
        "subagents": subagents,
    });
    if let Some(v) = session_id {
        p["session_id"] = json!(v);
    }
    if let Some(v) = started_at {
        p["started_at"] = json!(v);
    }
    if let Some(v) = finished_at {
        p["finished_at"] = json!(v);
    }
    if let Some(v) = label {
        p["label"] = json!(v);
    }
    call_of("spawn_tree.save", p)
}

/// Persist a delegation tree snapshot as JSON under the profile's spawn-trees dir. Write; leaves
/// a file behind (there is no delete RPC — remove it out of band).
pub async fn spawn_tree_save(
    client: &Client,
    subagents: Vec<Value>,
    session_id: Option<&str>,
    started_at: Option<f64>,
    finished_at: Option<f64>,
    label: Option<&str>,
    profile: &str,
) -> Result<SpawnTreeSaved, ApiError> {
    call_as(
        client,
        &spawn_tree_save_call(subagents, session_id, started_at, finished_at, label, profile),
    )
    .await
}

/// Builder for listing saved snapshots; `cross_session` widens to the whole spawn-trees root.
pub fn spawn_tree_list_call(session_id: Option<&str>, cross_session: bool, limit: Option<u32>, profile: &str) -> Call {
    let mut p = json!({
        "profile": profile_param(profile),
        "cross_session": cross_session,
    });
    if let Some(v) = session_id {
        p["session_id"] = json!(v);
    }
    if let Some(v) = limit {
        p["limit"] = json!(v);
    }
    call_of("spawn_tree.list", p)
}

/// List saved spawn-tree snapshots. Read-only.
pub async fn spawn_tree_list(
    client: &Client,
    session_id: Option<&str>,
    cross_session: bool,
    limit: Option<u32>,
    profile: &str,
) -> Result<SpawnTreeList, ApiError> {
    call_as(client, &spawn_tree_list_call(session_id, cross_session, limit, profile)).await
}

/// Builder for loading one snapshot; `path` must lie under the spawn-trees root.
pub fn spawn_tree_load_call(path: &str, profile: &str) -> Call {
    call_of(
        "spawn_tree.load",
        json!({ "profile": profile_param(profile), "path": path }),
    )
}

/// Read one saved snapshot. Read-only; rejects paths outside the spawn-trees root.
pub async fn spawn_tree_load(client: &Client, path: &str, profile: &str) -> Result<SpawnTreeSnapshot, ApiError> {
    call_as(client, &spawn_tree_load_call(path, profile)).await
}

// ── Handoff ──────────────────────────────────────────────────────────────────

/// `handoff.state`: where the session's handoff row stands.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct HandoffState {
    pub state: String,
    pub platform: String,
    pub error: String,
}

/// `handoff.request`: the queued handoff's platform home.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct HandoffQueued {
    pub queued: bool,
    pub session_key: String,
    pub platform: String,
    pub home_name: String,
}

/// `handoff.fail`: whether the CAS against the watcher won.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct HandoffFailed {
    pub failed: bool,
    pub state: String,
}

pub fn handoff_state_call(session_id: &str, profile: &str) -> Call {
    call_of(
        "handoff.state",
        json!({ "profile": profile_param(profile), "session_id": session_id }),
    )
}

/// Poll this session's handoff row. Read-only; 4001 when the session is unknown.
pub async fn handoff_state(client: &Client, session_id: &str, profile: &str) -> Result<HandoffState, ApiError> {
    call_as(client, &handoff_state_call(session_id, profile)).await
}

/// Builder for queueing a handoff to a messaging platform's home channel.
pub fn handoff_request_call(session_id: &str, platform: &str, profile: &str) -> Call {
    call_of(
        "handoff.request",
        json!({
            "profile": profile_param(profile),
            "session_id": session_id,
            "platform": platform,
        }),
    )
}

/// Queue a handoff (the gateway watcher claims it). **Network** — posts to the platform; confirm.
pub async fn handoff_request(
    client: &Client,
    session_id: &str,
    platform: &str,
    profile: &str,
) -> Result<HandoffQueued, ApiError> {
    call_as(client, &handoff_request_call(session_id, platform, profile)).await
}

/// Builder for failing a not-yet-claimed handoff after a client poll timeout.
pub fn handoff_fail_call(session_id: &str, error: Option<&str>, profile: &str) -> Call {
    let mut p = json!({ "profile": profile_param(profile), "session_id": session_id });
    if let Some(v) = error {
        p["error"] = json!(v);
    }
    call_of("handoff.fail", p)
}

/// Fail a still-pending handoff. Write; CAS against the watcher.
pub async fn handoff_fail(
    client: &Client,
    session_id: &str,
    error: Option<&str>,
    profile: &str,
) -> Result<HandoffFailed, ApiError> {
    call_as(client, &handoff_fail_call(session_id, error, profile)).await
}

// ── Wake word ────────────────────────────────────────────────────────────────

/// The resolved capture device (or why none resolved).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct WakeInputDevice {
    pub selector: Option<Value>,
    pub name: Option<String>,
    pub error: Option<String>,
    pub max_input_channels: Option<i64>,
    pub default_samplerate: Option<f64>,
    pub hostapi_index: Option<i64>,
    pub hostapi: Option<String>,
}

/// `wake.status`: everything needed to draw the wake-word state and decide whether to (re)arm.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct WakeStatus {
    pub listening: bool,
    pub owned_by_caller: bool,
    pub owner_surface: Option<String>,
    pub phrase: String,
    pub provider: String,
    pub configured_surface: String,
    pub input_device: WakeInputDevice,
    pub available: bool,
    pub hint: String,
    pub enabled: bool,
    pub audio_silent: bool,
    pub capture: String,
    pub local_input_available: bool,
    pub sample_rate: i64,
    pub frame_length: i64,
}

pub fn wake_status_call(surface: Option<&str>, client_capture: Option<bool>, profile: &str) -> Call {
    let mut p = json!({ "profile": profile_param(profile) });
    if let Some(v) = surface {
        p["surface"] = json!(v);
    }
    if let Some(v) = client_capture {
        p["client_capture"] = json!(v);
    }
    call_of("wake.status", p)
}

/// Read the wake-word state (device, phrase, ownership). Read-only; no capture is started.
pub async fn wake_status(
    client: &Client,
    surface: Option<&str>,
    client_capture: Option<bool>,
    profile: &str,
) -> Result<WakeStatus, ApiError> {
    call_as(client, &wake_status_call(surface, client_capture, profile)).await
}

/// `wake.start`: the arm result. `reason` is `unavailable` | `disabled` |
/// `disabled_for_surface` | `owned` when `started` is false.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct WakeStart {
    pub started: bool,
    pub reason: Option<String>,
    /// Why the surface cannot arm (`unavailable`), from the requirements probe.
    pub hint: Option<String>,
    /// `local` | `client` — `client` means the desktop feeds PCM through [`wake_feed_call`].
    pub capture: Option<String>,
    pub phrase: Option<String>,
    pub provider: Option<String>,
    pub owner_surface: Option<String>,
    /// True when this explicit gesture wrote `wake_word.enabled: true`.
    pub enabled_persisted: Option<bool>,
    pub sample_rate: Option<i64>,
    pub frame_length: Option<i64>,
}

/// `wake.start`: arm the wake-word listener for `surface` (`"tui"` | `"gui"`, empty = the
/// backend's `auto`). `persist: true` — an explicit gesture — also flips `wake_word.enabled`
/// on; auto-arm callers leave it off. `client_capture` asks for the client-PCM path
/// (`capture: "client"`), fed later with [`wake_feed_call`].
///
/// **Spawns/records** — opens the microphone on the backend host; builder + unit test only.
pub fn wake_start_call(
    surface: &str,
    persist: bool,
    session_id: &str,
    client_capture: Option<bool>,
    profile: &str,
) -> Call {
    let mut p = json!({ "profile": profile_param(profile) });
    if !surface.is_empty() {
        p["surface"] = json!(surface);
    }
    if persist {
        p["persist"] = json!(true);
    }
    if !session_id.is_empty() {
        p["session_id"] = json!(session_id);
    }
    if let Some(v) = client_capture {
        p["client_capture"] = json!(v);
    }
    call_of("wake.start", p)
}

/// Arm the wake-word listener. **Spawns/records the microphone** on the backend host.
pub async fn wake_start(
    client: &Client,
    surface: &str,
    persist: bool,
    session_id: &str,
    client_capture: Option<bool>,
    profile: &str,
) -> Result<WakeStart, ApiError> {
    call_as(
        client,
        &wake_start_call(surface, persist, session_id, client_capture, profile),
    )
    .await
}

/// `wake.stop`: stopping this surface's listener; `persist: true` also clears the config flag.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct WakeStop {
    pub stopped: bool,
    /// `null` when the caller owned the listener; `"not_owner"` otherwise.
    pub reason: Option<String>,
    /// True when this call wrote `wake_word.enabled: false`.
    pub disabled_persisted: bool,
}

/// `wake.stop`: stop this surface's listener; `persist: true` (an explicit gesture) also
/// writes `wake_word.enabled: false`.
///
/// **Releases the microphone** on the backend host; builder + unit test only.
pub fn wake_stop_call(persist: bool, profile: &str) -> Call {
    let mut p = json!({ "profile": profile_param(profile) });
    if persist {
        p["persist"] = json!(true);
    }
    call_of("wake.stop", p)
}

/// Stop the wake-word listener. **Releases the backend host's microphone.**
pub async fn wake_stop(client: &Client, persist: bool, profile: &str) -> Result<WakeStop, ApiError> {
    call_as(client, &wake_stop_call(persist, profile)).await
}

/// `wake.pause`: releasing the mic (e.g. while the desktop's browser captures audio).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct WakePause {
    pub paused: bool,
    /// `null` when the caller owned the listener; `"not_owner"` otherwise.
    pub reason: Option<String>,
}

/// `wake.pause`: release the mic. **Releases the backend host's microphone.**
pub fn wake_pause_call(profile: &str) -> Call {
    call_of("wake.pause", json!({ "profile": profile_param(profile) }))
}

/// Pause the wake-word detector.
pub async fn wake_pause(client: &Client, profile: &str) -> Result<WakePause, ApiError> {
    call_as(client, &wake_pause_call(profile)).await
}

/// `wake.resume`: reclaiming the mic after a pause; a no-op if the listener is not armed.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct WakeResume {
    pub resumed: bool,
    /// `null` when the caller owned the listener; `"not_owner"` otherwise.
    pub reason: Option<String>,
}

/// `wake.resume`: reclaim the mic after [`wake_pause`].
pub fn wake_resume_call(profile: &str) -> Call {
    call_of("wake.resume", json!({ "profile": profile_param(profile) }))
}

/// Resume the wake-word detector.
pub async fn wake_resume(client: &Client, profile: &str) -> Result<WakeResume, ApiError> {
    call_as(client, &wake_resume_call(profile)).await
}

/// `wake.feed`: pushing client-captured PCM into the armed detector (`reason: "empty"` for an
/// empty frame).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct WakeFeed {
    pub fed: bool,
    /// `null` when the caller owned the listener; `"not_owner"` / `"empty"` otherwise.
    pub reason: Option<String>,
}

/// `wake.feed`: push client-captured PCM (base64 int16 mono LE, 16 kHz only) into the armed
/// detector — mic-less remote backends run openWakeWord this way. `sample_rate` is optional;
/// the backend accepts `0` or `16000`.
///
/// **Records the microphone** on the calling surface; builder + unit test only.
pub fn wake_feed_call(pcm_b64: &str, sample_rate: Option<i64>, profile: &str) -> Call {
    let mut p = json!({ "profile": profile_param(profile), "pcm": pcm_b64 });
    if let Some(v) = sample_rate {
        p["sample_rate"] = json!(v);
    }
    call_of("wake.feed", p)
}

/// Push a client-captured PCM frame. **Feeds the backend host's detector.**
pub async fn wake_feed(
    client: &Client,
    pcm_b64: &str,
    sample_rate: Option<i64>,
    profile: &str,
) -> Result<WakeFeed, ApiError> {
    call_as(client, &wake_feed_call(pcm_b64, sample_rate, profile)).await
}

// ── Onboarding ───────────────────────────────────────────────────────────────

/// `onboarding.state`: whether the first-run flow should show, and the run it would resume.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct OnboardingState {
    /// The guided run should start (fresh install, or nothing finished yet).
    pub run: bool,
    /// This install is eligible for the guided run.
    pub eligible: bool,
    /// The live run's session, when one is open.
    pub session_id: Option<String>,
    pub run_id: Option<String>,
}

/// `onboarding.state`: `assume_fresh` answers as a first install (the dialog's preview).
/// Read-only; `_run_state` only reads the onboarding store. Its strict param model
/// (`OnboardingStateParams`) takes `assume_fresh` only — a `profile` is rejected with 4000.
pub fn onboarding_state_call(assume_fresh: bool) -> Call {
    let mut p = json!({});
    if assume_fresh {
        p["assume_fresh"] = json!(true);
    }
    call_of("onboarding.state", p)
}

/// Read whether the first-run chat should show. Read-only.
pub async fn onboarding_state(client: &Client, assume_fresh: bool) -> Result<OnboardingState, ApiError> {
    call_as(client, &onboarding_state_call(assume_fresh)).await
}

/// `onboarding.start` reply: the chat that was opened (or handed back).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct OnboardingStarted {
    pub session_id: String,
    pub stored_session_id: String,
    /// True when an already-stored run was resumed instead of a new chat created.
    pub resumed: bool,
    pub run_id: String,
}

/// `onboarding.start`: open (or resume) the first-run chat. `fresh_launch` discards runs a
/// killed app left behind; `rerun` starts over on the configured model. `facts_source` is
/// `local` | `remote`. The handler submits the opening prompt through `prompt.submit`. Its
/// strict param model takes no `profile` (the run is the default profile's).
///
/// **Spends model calls** — builder + unit test only.
pub fn onboarding_start_call(
    fresh_launch: bool,
    rerun: bool,
    assume_fresh: bool,
    locale: &str,
    facts_source: &str,
) -> Call {
    let mut p = json!({});
    if fresh_launch {
        p["fresh_launch"] = json!(true);
    }
    if rerun {
        p["rerun"] = json!(true);
    }
    if assume_fresh {
        p["assume_fresh"] = json!(true);
    }
    if !locale.is_empty() {
        p["locale"] = json!(locale);
    }
    if !facts_source.is_empty() {
        p["facts_source"] = json!(facts_source);
    }
    call_of("onboarding.start", p)
}

/// Start (or resume) the first-run chat. **Spends model calls.**
pub async fn onboarding_start(
    client: &Client,
    fresh_launch: bool,
    rerun: bool,
    assume_fresh: bool,
    locale: &str,
    facts_source: &str,
) -> Result<OnboardingStarted, ApiError> {
    call_as(
        client,
        &onboarding_start_call(fresh_launch, rerun, assume_fresh, locale, facts_source),
    )
    .await
}

/// `onboarding.skip` reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct OnboardingSkipped {
    /// True when the whole guided tour was skipped (`skipped_all`), not just one step.
    pub skipped: bool,
}

/// `onboarding.skip`: advance the live run's onboarding step to `skipped` and cancel its setup
/// card. Needs the run's live session and writes its stored record, so it is builder + unit
/// test only.
pub fn onboarding_skip_call(session_id: &str, profile: &str) -> Call {
    call_of(
        "onboarding.skip",
        json!({ "profile": profile_param(profile), "session_id": session_id }),
    )
}

/// Skip the live run's onboarding step. **Writes the run record.**
pub async fn onboarding_skip(client: &Client, session_id: &str, profile: &str) -> Result<OnboardingSkipped, ApiError> {
    call_as(client, &onboarding_skip_call(session_id, profile)).await
}

// ── Diagnostics & CLI ────────────────────────────────────────────────────────

/// `diagnostics.share_nous`: upload result.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct DiagnosticsShare {
    pub ok: bool,
    pub view_url: Option<String>,
    pub upload_id: Option<String>,
    pub expires_at: Option<String>,
    pub error: Option<String>,
}

/// `cli.exec`: captured `hermes <argv>` output.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct CliExec {
    pub blocked: bool,
    pub code: i64,
    pub output: String,
    pub hint: Option<String>,
}

/// Builder for uploading a force-redacted debug bundle to Nous diagnostics storage.
/// **Network** — leaves the machine.
pub fn diagnostics_share_nous_call(
    error_context: Option<&str>,
    extra_files: Option<&BTreeMap<String, String>>,
    log_lines: Option<u32>,
) -> Call {
    let mut p = json!({});
    if let Some(v) = error_context {
        p["error_context"] = json!(v);
    }
    if let Some(v) = extra_files {
        p["extra_files"] = json!(v);
    }
    if let Some(v) = log_lines {
        p["log_lines"] = json!(v);
    }
    call_of("diagnostics.share_nous", p)
}

/// Upload a redacted diagnostic bundle. **Network** — builder only until a UI needs it.
pub async fn diagnostics_share_nous(
    client: &Client,
    error_context: Option<&str>,
    extra_files: Option<&BTreeMap<String, String>>,
    log_lines: Option<u32>,
) -> Result<DiagnosticsShare, ApiError> {
    call_as(
        client,
        &diagnostics_share_nous_call(error_context, extra_files, log_lines),
    )
    .await
}

/// Builder for running `hermes <argv>` non-interactively.
/// **Spawns a process** — arbitrary CLI commands, refuse to automate blindly.
pub fn cli_exec_call(argv: &[String], timeout: Option<u32>, profile: &str) -> Call {
    let mut p = json!({ "argv": argv, "profile": profile_param(profile) });
    if let Some(v) = timeout {
        p["timeout"] = json!(v);
    }
    call_of("cli.exec", p)
}

/// Run a `hermes` CLI command and capture its output. **Spawns a process** — builder only.
pub async fn cli_exec(
    client: &Client,
    argv: &[String],
    timeout: Option<u32>,
    profile: &str,
) -> Result<CliExec, ApiError> {
    call_as(client, &cli_exec_call(argv, timeout, profile)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::decode;

    fn body(r: &Request) -> Option<Value> {
        r.body.clone()
    }

    #[test]
    fn rest_requests() {
        let r = system_status_request("");
        assert_eq!(
            (r.method, r.path.as_str(), body(&r)),
            (Method::Get, "/api/status", None)
        );
        assert_eq!(system_status_request("swe").path, "/api/status?profile=swe");

        let r = logs_request("agent", 50, Some("error"), None, Some("boom"), "swe");
        assert_eq!(
            (r.method, r.path.as_str()),
            (
                Method::Get,
                "/api/logs?file=agent&lines=50&level=error&search=boom&profile=swe"
            )
        );
        assert_eq!(
            logs_request("gateway", 5, None, None, None, "").path,
            "/api/logs?file=gateway&lines=5"
        );

        let r = env_request("swe");
        assert_eq!(
            (r.method, r.path.as_str(), body(&r)),
            (Method::Get, "/api/env?profile=swe", None)
        );

        let r = reveal_env_request("OPENAI_API_KEY", "");
        assert_eq!((r.method, r.path.as_str()), (Method::Post, "/api/env/reveal"));
        assert_eq!(body(&r), Some(json!({ "key": "OPENAI_API_KEY" })));

        let r = ops_doctor_request("");
        assert_eq!(
            (r.method, r.path.as_str(), body(&r)),
            (Method::Post, "/api/ops/doctor", None)
        );
        let r = ops_security_audit_request("swe");
        assert_eq!(
            (r.method, r.path.as_str()),
            (Method::Post, "/api/ops/security-audit?profile=swe")
        );

        let r = ops_backup_request(None, "");
        assert_eq!((r.method, r.path.as_str()), (Method::Post, "/api/ops/backup"));
        assert_eq!(body(&r), Some(json!({})));
        assert_eq!(
            body(&ops_backup_request(Some("/tmp/x.zip"), "")),
            Some(json!({ "output": "/tmp/x.zip" }))
        );

        let r = ops_debug_share_request(200, true, "swe");
        assert_eq!(
            (r.method, r.path.as_str(), body(&r)),
            (
                Method::Post,
                "/api/ops/debug-share?profile=swe",
                Some(json!({ "lines": 200, "redact": true }))
            )
        );

        let r = action_status_request("doctor", 50);
        assert_eq!(
            (r.method, r.path.as_str()),
            (Method::Get, "/api/actions/doctor/status?lines=50")
        );
        assert_eq!(
            action_status_request("a b", 5).path,
            "/api/actions/a%20b/status?lines=5"
        );

        assert_eq!(
            update_check_request(true, "swe").path,
            "/api/hermes/update/check?force=true&profile=swe"
        );
        assert_eq!(update_check_request(false, "").path, "/api/hermes/update/check");
        let r = update_apply_request();
        assert_eq!(
            (r.method, r.path.as_str(), body(&r)),
            (Method::Post, "/api/hermes/update", None)
        );

        let r = gateway_restart_request("swe");
        assert_eq!(
            (r.method, r.path.as_str(), body(&r)),
            (Method::Post, "/api/gateway/restart?profile=swe", None)
        );

        assert_eq!(
            usage_analytics_request(30, "swe").path,
            "/api/analytics/usage?days=30&profile=swe"
        );

        assert_eq!(media_request("/tmp/a b.png").path, "/api/media?path=%2Ftmp%2Fa%20b.png");
        assert_eq!(
            media_proxy_request("https://x.test/a.png").path,
            "/api/media/proxy?url=https%3A%2F%2Fx.test%2Fa.png"
        );
    }

    #[test]
    fn rpc_calls() {
        assert_eq!(
            (config_show_call("").method, config_show_call("").params),
            ("config.show", json!({ "profile": null }))
        );
        assert_eq!(config_show_call("swe").params, json!({ "profile": "swe" }));

        assert_eq!(setup_status_call("").params, json!({ "profile": null }));
        assert_eq!(
            setup_runtime_check_call(None, "").params,
            json!({ "profile": null, "provider": null })
        );
        assert_eq!(
            setup_runtime_check_call(Some("openai"), "swe").params,
            json!({ "profile": "swe", "provider": "openai" })
        );

        assert_eq!(i18n_languages_call("").params, json!({ "profile": null }));
        assert_eq!(
            i18n_catalog_call("pt-BR", "desktop", "").params,
            json!({ "profile": null, "lang": "pt-BR", "surface": "desktop" })
        );

        assert_eq!(machine_facts_call().params, json!({}));
        assert_eq!(system_battery_call("swe").params, json!({ "profile": "swe" }));
        assert_eq!(gateway_capabilities_call().params, json!({}));
        assert_eq!(agents_list_call("").params, json!({ "profile": null }));

        assert_eq!(
            verification_status_call(None, None, None, "").params,
            json!({ "profile": null })
        );
        assert_eq!(
            verification_status_call(Some("s1"), Some("k1"), Some("/repo"), "swe").params,
            json!({ "profile": "swe", "session_id": "s1", "session_key": "k1", "cwd": "/repo" })
        );

        assert_eq!(
            insights_get_call(Some(30), "").params,
            json!({ "profile": null, "days": 30 })
        );
        assert_eq!(insights_get_call(None, "").params, json!({ "profile": null }));
        assert_eq!(usage_bars_call("").params, json!({ "profile": null }));

        assert_eq!(delegation_status_call("swe").params, json!({ "profile": "swe" }));
        assert_eq!(
            delegation_pause_call(true, "").params,
            json!({ "profile": null, "paused": true })
        );

        assert_eq!(
            subagent_list_call("s1", "").params,
            json!({ "profile": null, "session_id": "s1" })
        );
        assert_eq!(
            subagent_tail_call("s1", "sub", "").params,
            json!({ "profile": null, "session_id": "s1", "subagent_id": "sub" })
        );
        assert_eq!(
            subagent_steer_call("s1", "sub", "go", "").params,
            json!({ "profile": null, "session_id": "s1", "subagent_id": "sub", "text": "go" })
        );
        assert_eq!(
            subagent_interrupt_call("s1", "sub", "").params,
            json!({ "profile": null, "session_id": "s1", "subagent_id": "sub" })
        );

        assert_eq!(
            spawn_tree_save_call(
                vec![json!({ "subagent_id": "a" })],
                Some("s1"),
                Some(1.5),
                Some(2.5),
                Some("label"),
                ""
            )
            .params,
            json!({
                "profile": null,
                "subagents": [{ "subagent_id": "a" }],
                "session_id": "s1",
                "started_at": 1.5,
                "finished_at": 2.5,
                "label": "label"
            })
        );
        assert_eq!(
            spawn_tree_list_call(Some("s1"), true, Some(10), "").params,
            json!({ "profile": null, "cross_session": true, "session_id": "s1", "limit": 10 })
        );
        assert_eq!(
            spawn_tree_list_call(None, false, None, "swe").params,
            json!({ "profile": "swe", "cross_session": false })
        );
        assert_eq!(
            spawn_tree_load_call("/p", "").params,
            json!({ "profile": null, "path": "/p" })
        );

        assert_eq!(
            handoff_state_call("s1", "").params,
            json!({ "profile": null, "session_id": "s1" })
        );
        assert_eq!(
            handoff_request_call("s1", "telegram", "").params,
            json!({ "profile": null, "session_id": "s1", "platform": "telegram" })
        );
        assert_eq!(
            handoff_fail_call("s1", Some("timeout"), "").params,
            json!({ "profile": null, "session_id": "s1", "error": "timeout" })
        );

        assert_eq!(wake_status_call(None, None, "").params, json!({ "profile": null }));
        assert_eq!(
            wake_status_call(Some("desktop"), Some(true), "").params,
            json!({ "profile": null, "surface": "desktop", "client_capture": true })
        );
        assert_eq!(
            wake_start_call("gui", false, "", None, "").params,
            json!({ "profile": null, "surface": "gui" })
        );
        assert_eq!(
            wake_start_call("gui", true, "s1", Some(true), "swe").params,
            json!({ "profile": "swe", "surface": "gui", "persist": true, "session_id": "s1",
                    "client_capture": true })
        );
        assert_eq!(wake_stop_call(false, "").params, json!({ "profile": null }));
        assert_eq!(
            wake_stop_call(true, "swe").params,
            json!({ "profile": "swe", "persist": true })
        );
        assert_eq!(wake_pause_call("").params, json!({ "profile": null }));
        assert_eq!(wake_resume_call("swe").params, json!({ "profile": "swe" }));
        assert_eq!(
            wake_feed_call("AAA=", None, "").params,
            json!({ "profile": null, "pcm": "AAA=" })
        );
        assert_eq!(
            wake_feed_call("AAA=", Some(16000), "").params,
            json!({ "profile": null, "pcm": "AAA=", "sample_rate": 16000 })
        );

        assert_eq!(onboarding_state_call(false).params, json!({}));
        assert_eq!(onboarding_state_call(true).params, json!({ "assume_fresh": true }));
        assert_eq!(
            onboarding_start_call(true, false, false, "de", "local").params,
            json!({ "fresh_launch": true, "locale": "de", "facts_source": "local" })
        );
        assert_eq!(
            onboarding_start_call(false, true, true, "", "").params,
            json!({ "rerun": true, "assume_fresh": true })
        );
        assert_eq!(
            onboarding_skip_call("s1", "").params,
            json!({ "profile": null, "session_id": "s1" })
        );

        assert_eq!(diagnostics_share_nous_call(None, None, None).params, json!({}));
        let extra = BTreeMap::from([("app.log".to_string(), "hello".to_string())]);
        assert_eq!(
            diagnostics_share_nous_call(Some("boom"), Some(&extra), Some(50)).params,
            json!({ "error_context": "boom", "extra_files": { "app.log": "hello" }, "log_lines": 50 })
        );

        let argv = vec!["model".to_string(), "list".to_string()];
        assert_eq!(
            cli_exec_call(&argv, Some(30), "swe").params,
            json!({ "argv": ["model", "list"], "timeout": 30, "profile": "swe" })
        );
    }

    macro_rules! empty_defaults {
        ($($t:ty),* $(,)?) => {
            $( assert_eq!(decode::<$t>(json!({})).unwrap(), <$t>::default()); )*
        };
    }

    #[test]
    fn empty_payloads_are_lenient() {
        empty_defaults!(
            ComponentHealth,
            GatewayRow,
            SystemStatus,
            Logs,
            ProviderProfileRef,
            EnvVar,
            RevealedEnvVar,
            ActionStarted,
            BackupStarted,
            DebugShare,
            ActionStatus,
            UpdateCommit,
            UpdateCheck,
            UsageDay,
            UsageModelRow,
            UsageTotals,
            UsageSkills,
            UsageTool,
            UsageAnalytics,
            MediaData,
            ConfigSection,
            ConfigShow,
            SetupStatus,
            SetupRuntimeCheck,
            LanguageOption,
            I18nLanguages,
            I18nCatalog,
            MachineInfo,
            MachineFacts,
            BatteryStatus,
            GatewayCapabilities,
            AgentProcess,
            AgentsList,
            VerificationInfo,
            VerificationStatus,
            Insights,
            UsageBar,
            UsageBars,
            ActiveSubagent,
            DelegationStatus,
            DelegationPause,
            SubagentSnapshot,
            FailedDelegation,
            SubagentList,
            SubagentTail,
            SubagentSteer,
            SubagentInterrupt,
            SpawnTreeEntry,
            SpawnTreeList,
            SpawnTreeSaved,
            SpawnTreeSnapshot,
            HandoffState,
            HandoffQueued,
            HandoffFailed,
            WakeInputDevice,
            WakeStatus,
            WakeStart,
            WakeStop,
            WakePause,
            WakeResume,
            WakeFeed,
            OnboardingState,
            OnboardingStarted,
            OnboardingSkipped,
            DiagnosticsShare,
            CliExec,
        );
        let vars: EnvVars = decode(json!({})).unwrap();
        assert!(vars.is_empty());
    }

    #[test]
    fn decode_status() {
        let s: SystemStatus = decode(json!({
            "version": "0.14.0",
            "release_date": "2026-10-01",
            "config_version": 7,
            "latest_config_version": 7,
            "can_update_hermes": true,
            "gateway_running": true,
            "gateway_state": "running",
            "gateway_platforms": { "telegram": { "state": "connected" } },
            "gateway_exit_reason": null,
            "gateway_updated_at": 1760000000.5,
            "gateway_heartbeat_stale_s": null,
            "gateway_shared_with": ["swe"],
            "active_agents": 0,
            "gateway_busy": false,
            "gateway_drainable": true,
            "restart_drain_timeout": 30.0,
            "active_sessions": 2,
            "auth_required": false,
            "auth_providers": ["nous"],
            "auth_flows": ["cookie", "native_pkce"],
            "nous_session_valid": "valid",
            "install_id": "abc",
            "shared_profile_warning": false,
            "components": {
                "gateway": { "status": "ok", "state": "running" },
                "storage": { "status": "degraded", "reason": "corrupt" }
            },
            "overall": "degraded",
            "profiles": ["default", "swe"],
            "parked_profiles": [],
            "gateway_mode": "multiplex",
            "multiplex_standalone_reason": null,
            "hermes_home": "/home/x/.hermes",
            "config_path": "/home/x/.hermes/config.yaml",
            "env_path": "/home/x/.hermes/.env",
            "gateway_pid": 4242,
            "gateway_health_url": "http://127.0.0.1:7777/health",
            "gateways": [{ "profile": "default", "ports": [7777], "served_profiles": ["swe"] }],
            "future_field": { "ignored": true }
        }))
        .unwrap();
        assert_eq!(s.version, "0.14.0");
        assert_eq!(s.config_version, Some(7));
        assert_eq!(s.active_sessions, 2);
        assert_eq!(s.auth_flows, ["cookie", "native_pkce"]);
        assert_eq!(s.gateway_state.as_deref(), Some("running"));
        assert_eq!(s.nous_session_valid, "valid");
        assert_eq!(s.gateway_pid, Some(4242));
        assert_eq!(s.overall, "degraded");
        assert_eq!(s.components["storage"].status, "degraded");
        assert_eq!(s.components["storage"].reason.as_deref(), Some("corrupt"));
        assert_eq!(s.gateways[0].profile, "default");
        assert_eq!(s.gateways[0].served_profiles, ["swe"]);
        assert_eq!(s.gateway_mode, "multiplex");
    }

    #[test]
    fn decode_logs_env_media() {
        let l: Logs = decode(json!({ "file": "agent", "lines": ["a", "b"], "extra": 1 })).unwrap();
        assert_eq!(l.file, "agent");
        assert_eq!(l.lines.len(), 2);

        let vars: EnvVars = decode(json!({
            "OPENAI_API_KEY": {
                "is_set": true,
                "redacted_value": "redacted:abcd",
                "description": "OpenAI key",
                "url": "https://platform.openai.com",
                "category": "provider",
                "is_password": true,
                "tools": ["chat"],
                "advanced": false,
                "channel_managed": false,
                "provider": "openai",
                "provider_label": "OpenAI",
                "provider_profiles": [
                    { "provider": "openai", "provider_label": "OpenAI", "primary": true },
                    { "provider": "openai-codex", "provider_label": "OpenAI Codex" }
                ],
                "provider_primary": true,
                "custom": false
            },
            "MY_VAR": { "is_set": false }
        }))
        .unwrap();
        assert!(vars["OPENAI_API_KEY"].is_password);
        assert_eq!(vars["OPENAI_API_KEY"].redacted_value.as_deref(), Some("redacted:abcd"));
        assert_eq!(vars["OPENAI_API_KEY"].provider_profiles.len(), 2);
        assert_eq!(vars["OPENAI_API_KEY"].provider_profiles[0].provider, "openai");
        assert_eq!(vars["OPENAI_API_KEY"].provider_profiles[0].primary, Some(true));
        assert!(!vars["MY_VAR"].is_set);

        let r: RevealedEnvVar = decode(json!({ "key": "K", "value": "sk-secret" })).unwrap();
        assert_eq!(r.key, "K");
        let shown = format!("{r:?}");
        assert!(shown.contains("<redacted>"));
        assert!(!shown.contains("sk-secret"));

        let m: MediaData = decode(json!({ "data_url": "data:image/png;base64,AA" })).unwrap();
        assert!(m.data_url.starts_with("data:image/png"));
    }

    #[test]
    fn decode_actions_updates_usage() {
        let a: ActionStarted = decode(json!({
            "ok": true, "pid": 123, "name": "doctor", "action_id": "a1",
            "already_running": false, "HERMES_ACTION_ID": "x9"
        }))
        .unwrap();
        assert!(a.ok);
        assert_eq!(a.pid, Some(123));
        assert_eq!(a.hermes_action_id.as_deref(), Some("x9"));

        let b: BackupStarted = decode(json!({
            "ok": true, "pid": 1, "name": "backup", "archive": "/tmp/a.zip"
        }))
        .unwrap();
        assert_eq!(b.action.name, "backup");
        assert_eq!(b.archive.as_deref(), Some("/tmp/a.zip"));

        let d: DebugShare = decode(json!({
            "ok": true,
            "urls": { "summary": "https://paste.test/1" },
            "failures": [],
            "redacted": true,
            "auto_delete_seconds": 604800
        }))
        .unwrap();
        assert_eq!(d.urls["summary"], "https://paste.test/1");
        assert_eq!(d.auto_delete_seconds, Some(604800));

        let s: ActionStatus = decode(json!({
            "name": "doctor", "running": false, "exit_code": 0, "pid": 9,
            "lines": ["done"], "action_id": "a1", "receipt": { "outcome": "ok" }
        }))
        .unwrap();
        assert_eq!(s.exit_code, Some(0));
        assert_eq!(s.receipt.as_ref().unwrap()["outcome"], "ok");

        let u: UpdateCheck = decode(json!({
            "install_method": "git", "current_version": "0.14.0", "behind": 3,
            "update_available": true, "can_apply": true, "update_command": "hermes update",
            "message": null, "channel": "stable", "branch": "main", "source_version": "0.15.0",
            "commits": [{ "sha": "abc1234", "summary": "fix", "author": "a", "at": 1760000000 }]
        }))
        .unwrap();
        assert_eq!(u.behind, Some(3));
        assert_eq!(u.commits.len(), 1);
        assert_eq!(u.commits[0].sha, "abc1234");

        let a: UsageAnalytics = decode(json!({
            "daily": [{ "day": "2026-10-01", "input_tokens": 10, "output_tokens": 5,
                        "cache_read_tokens": 1, "reasoning_tokens": 0,
                        "estimated_cost": 0.01, "actual_cost": 0.0,
                        "sessions": 1, "api_calls": 2 }],
            "by_model": [{ "model": "deepseek", "input_tokens": 10, "output_tokens": 5,
                           "estimated_cost": 0.01, "sessions": 1, "api_calls": 2 }],
            "by_task": [{ "task": "compression", "sessions": 1 }],
            "totals": { "total_input": 10, "total_output": 5, "total_sessions": 1 },
            "period_days": 30,
            "skills": { "summary": { "total": 1 }, "top_skills": [{ "name": "x" }] },
            "tools": [{ "tool": "terminal", "count": 4, "percentage": 50.0 }]
        }))
        .unwrap();
        assert_eq!(a.period_days, 30);
        assert_eq!(a.daily[0].input_tokens, Some(10));
        assert_eq!(a.totals.as_ref().unwrap().total_sessions, Some(1));
        assert_eq!(a.tools[0].tool, "terminal");

        let empty_totals: UsageAnalytics = decode(json!({ "totals": null, "period_days": 7 })).unwrap();
        assert!(empty_totals.totals.is_none());
        assert_eq!(empty_totals.period_days, 7);
    }

    #[test]
    fn decode_rpc_payloads() {
        let c: ConfigShow = decode(json!({
            "sections": [{ "title": "Model", "rows": [["provider", "openai"], ["model", "gpt"]] }]
        }))
        .unwrap();
        assert_eq!(c.sections[0].rows[1][1], "gpt");

        let s: SetupStatus = decode(json!({
            "provider_configured": true, "ready": true, "free_tier_account": false,
            "free_tier_route": false, "other_providers": true, "inference_provider": "openai",
            "profile": "default", "ok": true, "error": null, "retryable": false
        }))
        .unwrap();
        assert_eq!(s.ready, Some(true));
        let rc: SetupRuntimeCheck = decode(json!({
            "ok": false, "error": "no provider", "provider": null, "model": null,
            "source": null, "free_tier_route": false, "profile": "swe"
        }))
        .unwrap();
        assert!(!rc.ok);
        assert_eq!(rc.error.as_deref(), Some("no provider"));

        let l: I18nLanguages = decode(json!({
            "languages": [{ "id": "en", "endonym": "English", "rtl": false, "source": "bundled" }]
        }))
        .unwrap();
        assert_eq!(l.languages[0].id, "en");
        let cat: I18nCatalog = decode(json!({
            "lang": "pt-BR", "surface": "desktop", "messages": { "hello": "Olá" }
        }))
        .unwrap();
        assert_eq!(cat.messages["hello"], "Olá");

        let m: MachineFacts = decode(json!({
            "machine": { "os_family": "linux", "os_release": "7.2.5", "native_arch": "x86_64",
                         "cpu_model": "Ryzen", "ram_gb": 32, "gpu_class": "discrete",
                         "vendor": "LENOVO", "wsl": false, "container": false },
            "machine_kind": "laptop", "has_nvidia_gpu": true,
            "nvidia_platform": "discrete", "locale": "en_US"
        }))
        .unwrap();
        assert_eq!(m.machine.ram_gb, Some(32));
        assert!(m.has_nvidia_gpu);

        let b: BatteryStatus = decode(json!({
            "available": true, "percent": 83, "plugged": true, "category": "good"
        }))
        .unwrap();
        assert_eq!(b.percent, Some(83));

        let cap: GatewayCapabilities = decode(json!({ "per_session_exclusive_submit": true })).unwrap();
        assert!(cap.per_session_exclusive_submit);

        let a: AgentsList = decode(json!({
            "processes": [{ "session_id": "s1", "command": "hermes chat", "status": "running", "uptime": 12 }]
        }))
        .unwrap();
        assert_eq!(a.processes[0].uptime, 12);

        let v: VerificationStatus = decode(json!({
            "verification": {
                "status": "verified", "evidence": { "id": 1, "command": "cargo test" },
                "root": "/repo", "session_id": "s1", "changed_paths": ["src/lib.rs"]
            }
        }))
        .unwrap();
        assert_eq!(v.verification.status, "verified");
        assert_eq!(v.verification.changed_paths.as_ref().unwrap().len(), 1);

        let i: Insights = decode(json!({ "days": 30, "sessions": 4, "messages": 120 })).unwrap();
        assert_eq!(i.messages, 120);

        let bars: UsageBars = decode(json!({
            "ok": true, "available": true, "status": "active", "plan_name": "Pro",
            "renews_at": "2026-11-01", "renews_display": "Nov 1", "has_topup": true,
            "subscription_remaining_display": "$10", "topup_remaining_display": "$5",
            "total_spendable_display": "$15",
            "plan_bar": { "kind": "plan", "remaining_display": "$10", "total_display": "$20",
                          "spent_display": "$10", "pct_used": 50, "fill_fraction": 0.5 },
            "topup_bar": null
        }))
        .unwrap();
        assert_eq!(bars.plan_bar.as_ref().unwrap().pct_used, Some(50));
        assert!(bars.topup_bar.is_none());

        let d: DelegationStatus = decode(json!({
            "active": [{ "subagent_id": "sub1", "parent_id": null, "depth": 1,
                         "goal": "build", "status": "running", "tool_count": 3 }],
            "paused": false, "max_spawn_depth": 3, "max_concurrent_children": 5
        }))
        .unwrap();
        assert_eq!(d.active[0].subagent_id, "sub1");
        assert_eq!(d.max_concurrent_children, 5);
        let p: DelegationPause = decode(json!({ "paused": true })).unwrap();
        assert!(p.paused);

        let sl: SubagentList = decode(json!({
            "subagents": [{ "subagent_id": "sub1", "status": "running",
                            "last_tool": "terminal", "accepting_steer": true }],
            "delegations": [{ "delegation_id": "d1", "task_index": 0, "status": "failed",
                              "goal": "g", "error": "boom" }]
        }))
        .unwrap();
        assert_eq!(sl.subagents[0].last_tool.as_deref(), Some("terminal"));
        assert_eq!(sl.delegations[0].task_index, 0);

        let t: SubagentTail = decode(json!({
            "subagent_id": "sub1", "available": true, "text": "hi", "truncated": false
        }))
        .unwrap();
        assert!(t.available);
        let st: SubagentSteer = decode(json!({
            "status": "queued", "subagent_id": "sub1", "text": "go"
        }))
        .unwrap();
        assert_eq!(st.status, "queued");
        let it: SubagentInterrupt = decode(json!({ "found": false, "subagent_id": "sub1" })).unwrap();
        assert!(!it.found);

        let saved: SpawnTreeSaved = decode(json!({ "path": "/trees/a.json", "session_id": "s1" })).unwrap();
        assert_eq!(saved.session_id, "s1");
        let list: SpawnTreeList = decode(json!({
            "entries": [{ "path": "/trees/a.json", "session_id": "s1",
                          "started_at": 1.0, "finished_at": 2.0, "label": "run", "count": 2 }]
        }))
        .unwrap();
        assert_eq!(list.entries[0].count, 2);
        let snap: SpawnTreeSnapshot = decode(json!({
            "session_id": "s1", "started_at": 1.0, "finished_at": 2.0, "label": "run",
            "subagents": [{ "subagent_id": "a" }]
        }))
        .unwrap();
        assert_eq!(snap.subagents.len(), 1);

        let hs: HandoffState = decode(json!({
            "state": "pending", "platform": "telegram", "error": ""
        }))
        .unwrap();
        assert_eq!(hs.state, "pending");
        let hq: HandoffQueued = decode(json!({
            "queued": true, "session_key": "k1", "platform": "telegram", "home_name": "Home"
        }))
        .unwrap();
        assert!(hq.queued);
        let hf: HandoffFailed = decode(json!({ "failed": true, "state": "failed" })).unwrap();
        assert!(hf.failed);

        let w: WakeStatus = decode(json!({
            "listening": false, "owned_by_caller": false, "owner_surface": null,
            "phrase": "hey hermes", "provider": "openwakeword", "configured_surface": "tui",
            "input_device": { "selector": 3, "name": "Mic", "error": null,
                              "max_input_channels": 2, "default_samplerate": 48000.0,
                              "hostapi_index": 0, "hostapi": "ALSA" },
            "available": true, "hint": "", "enabled": true, "audio_silent": false,
            "capture": "server", "local_input_available": true,
            "sample_rate": 16000, "frame_length": 1280
        }))
        .unwrap();
        assert_eq!(w.phrase, "hey hermes");
        assert_eq!(w.input_device.name.as_deref(), Some("Mic"));

        let ds: DiagnosticsShare = decode(json!({
            "ok": true, "view_url": "https://diag.test/v/1", "upload_id": "u1",
            "expires_at": "2026-11-01", "error": null
        }))
        .unwrap();
        assert_eq!(ds.upload_id.as_deref(), Some("u1"));

        let ce: CliExec = decode(json!({
            "blocked": false, "code": 0, "output": "ok", "hint": null
        }))
        .unwrap();
        assert_eq!(ce.code, 0);
        assert!(!ce.blocked);

        let ws: WakeStart = decode(json!({
            "started": false, "reason": "unavailable", "hint": "no input device",
            "capture": "client", "phrase": "hey hermes", "provider": "openwakeword",
            "owner_surface": "gui", "enabled_persisted": false,
            "sample_rate": 16000, "frame_length": 1280
        }))
        .unwrap();
        assert_eq!(ws.reason.as_deref(), Some("unavailable"));
        assert_eq!(ws.capture.as_deref(), Some("client"));
        let wstop: WakeStop = decode(json!({
            "stopped": true, "reason": null, "disabled_persisted": false
        }))
        .unwrap();
        assert!(wstop.stopped);
        let wpause: WakePause = decode(json!({"paused": false, "reason": "not_owner"})).unwrap();
        assert_eq!(wpause.reason.as_deref(), Some("not_owner"));
        let wresume: WakeResume = decode(json!({"resumed": true, "reason": null})).unwrap();
        assert!(wresume.resumed);
        let wfeed: WakeFeed = decode(json!({"fed": false, "reason": "empty"})).unwrap();
        assert_eq!(wfeed.reason.as_deref(), Some("empty"));

        let os: OnboardingState = decode(json!({
            "run": true, "eligible": true, "session_id": "s1", "run_id": "r1"
        }))
        .unwrap();
        assert!(os.run && os.eligible);
        assert_eq!(os.session_id.as_deref(), Some("s1"));
        let ostarted: OnboardingStarted = decode(json!({
            "session_id": "s1", "stored_session_id": "st1", "resumed": false, "run_id": "r1"
        }))
        .unwrap();
        assert_eq!(ostarted.stored_session_id, "st1");
        assert!(!ostarted.resumed);
        let oskip: OnboardingSkipped = decode(json!({"skipped": false})).unwrap();
        assert!(!oskip.skipped);
    }
}

// ── Process liveness, WS ticket, ops hooks, checkpoints ───────────────────────

/// `GET /api/health`: the desktop's readiness probe. `commit` is the code the
/// process **booted** from, so the caller rejects a backend left over from an
/// older checkout. Public (no token) on a normal loopback backend.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct Health {
    pub ok: bool,
    pub version: String,
    /// The backend spells this `displayVersion`.
    #[serde(rename = "displayVersion")]
    pub display_version: String,
    pub commit: String,
    pub auth_required: bool,
}

pub fn health_request() -> Request {
    req(Method::Get, "/api/health", None)
}

/// Lightweight process liveness, version and boot commit (read-only).
pub async fn health(rest: &Rest) -> Result<Health, ApiError> {
    send_as(rest, &health_request()).await
}

/// `POST /api/auth/ws-ticket` reply. `ticket` is a single-use 30 s credential
/// passed as `?ticket=` on the WebSocket upgrade. `Debug` prints only its length.
#[derive(Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct WsTicket {
    pub ticket: String,
    pub ttl_seconds: u64,
}

impl fmt::Debug for WsTicket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WsTicket")
            .field("ticket", &format!("<{} chars>", self.ticket.len()))
            .field("ttl_seconds", &self.ttl_seconds)
            .finish()
    }
}

impl WsTicket {
    /// Length of the credential, for logs that must never print it.
    pub fn ticket_len(&self) -> usize {
        self.ticket.len()
    }
}

pub fn ws_ticket_request() -> Request {
    req(Method::Post, "/api/auth/ws-ticket", None)
}

/// Mint a single-use WebSocket upgrade ticket (gated mode only). **Reveals a
/// credential** — never log the value; pass it straight to the WS URL.
pub async fn ws_ticket(rest: &Rest) -> Result<WsTicket, ApiError> {
    send_as(rest, &ws_ticket_request()).await
}

/// One configured shell hook (`GET /api/ops/hooks`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct HookSpec {
    pub event: String,
    pub matcher: Option<String>,
    pub command: String,
    pub timeout: Option<f64>,
    /// Present in the allowlist (`hermes hooks allow`).
    pub allowed: bool,
    pub approved_at: Option<String>,
    /// The script is currently executable on the backend host.
    pub executable: bool,
}

/// `GET /api/ops/hooks`: configured hooks plus the valid event names for the form.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct OpsHooks {
    pub hooks: Vec<HookSpec>,
    pub valid_events: Vec<String>,
}

pub fn ops_hooks_request(profile: &str) -> Request {
    req(Method::Get, format!("/api/ops/hooks{}", profile_query(profile)), None)
}

/// Read the profile's configured shell hooks and consent status (read-only).
pub async fn ops_hooks(rest: &Rest, profile: &str) -> Result<OpsHooks, ApiError> {
    send_as(rest, &ops_hooks_request(profile)).await
}

pub fn checkpoints_prune_request(profile: &str) -> Request {
    req(
        Method::Post,
        format!("/api/ops/checkpoints/prune{}", explicit_profile_query(profile)),
        None,
    )
}

/// Prune `/rollback` checkpoints. **Destructive** — spawned as a background
/// action (`checkpoints-prune`); poll [`action_status`]. Builder-only.
pub async fn checkpoints_prune(rest: &Rest, profile: &str) -> Result<ActionStarted, ApiError> {
    send_as(rest, &checkpoints_prune_request(profile)).await
}

#[cfg(test)]
mod rest_gap_tests {
    use super::*;
    use crate::api::decode;

    #[test]
    fn health_and_ticket_builders() {
        let r = health_request();
        assert_eq!((r.method, r.path.as_str(), r.body), (Method::Get, "/api/health", None));
        let h: Health = decode(json!({
            "ok": true, "version": "0.19.1", "displayVersion": "0.19.1 (abc)",
            "commit": "abc123", "auth_required": false
        }))
        .unwrap();
        assert!(h.ok);
        assert_eq!(h.display_version, "0.19.1 (abc)");
        assert_eq!(h.commit, "abc123");
        assert_eq!(decode::<Health>(json!({})).unwrap(), Health::default());

        let r = ws_ticket_request();
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (Method::Post, "/api/auth/ws-ticket", None)
        );
        let t: WsTicket = decode(json!({"ticket": "abc", "ttl_seconds": 30})).unwrap();
        assert_eq!(t.ticket_len(), 3);
        assert_eq!(format!("{t:?}"), "WsTicket { ticket: \"<3 chars>\", ttl_seconds: 30 }");
        assert_eq!(decode::<WsTicket>(json!({})).unwrap(), WsTicket::default());
    }

    #[test]
    fn hooks_and_prune_builders() {
        assert_eq!(ops_hooks_request("").path, "/api/ops/hooks");
        assert_eq!(ops_hooks_request("swe").path, "/api/ops/hooks?profile=swe");
        let h: OpsHooks = decode(json!({
            "hooks": [{"event": "pre_tool", "matcher": null, "command": "~/hook.sh",
                       "timeout": 5.5, "allowed": true, "approved_at": "2026-10-01T00:00:00Z",
                       "executable": true}],
            "valid_events": ["pre_tool", "post_tool"]
        }))
        .unwrap();
        assert_eq!(h.hooks.len(), 1);
        assert_eq!(h.hooks[0].timeout, Some(5.5));
        assert!(h.hooks[0].allowed);
        assert_eq!(h.valid_events, vec!["pre_tool", "post_tool"]);
        assert_eq!(decode::<OpsHooks>(json!({})).unwrap(), OpsHooks::default());

        let r = checkpoints_prune_request("");
        assert_eq!(
            (r.method, r.path.as_str()),
            (Method::Post, "/api/ops/checkpoints/prune?profile=default")
        );
        assert_eq!(
            checkpoints_prune_request("swe").path,
            "/api/ops/checkpoints/prune?profile=swe"
        );
        let a: ActionStarted = decode(json!({"ok": true, "pid": 42, "name": "checkpoints-prune"})).unwrap();
        assert!(a.ok);
        assert_eq!(a.pid, Some(42));
    }
}
