//! Capability extras beyond the Capabilities screen: skills hub and skills.manage, tools/toolsets RPCs and config panels, terminal backends, computer use, MCP RPCs/OAuth/catalog, cron blueprints and delivery targets, profile extras (active, import/export, describe/configure, assets), memory provider config/OAuth, learning graph, vault.
//!
//! Follows the conventions in [`crate::api`].
//!
//! Split in two files: this one = skills hub, tools/toolsets, terminal backends, computer use,
//! MCP RPCs + catalog, learning, vault. [`admin`] = cron extras, profile extras, memory providers.

pub mod admin;

use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::{ApiError, Call, Method, Request, call, call_as, call_of, profile_query, req, seg, send_as, with_query};
use crate::http::Rest;
use crate::rpc::Client;

/// Add `profile` to a JSON body when the profile is not the launch default.
fn body_with_profile(mut body: Value, profile: &str) -> Value {
    if !profile.is_empty() {
        body["profile"] = json!(profile);
    }
    body
}

/// Add a non-empty string parameter to an RPC params object.
fn put_str(params: &mut Value, key: &str, value: &str) {
    if !value.is_empty() {
        params[key] = json!(value);
    }
}

// ── Skills hub ──────────────────────────────────────────────────────────────────

/// One hub skill row (official catalog listing or search result).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct HubSkill {
    pub name: String,
    pub description: String,
    pub source: String,
    pub identifier: String,
    pub trust_level: String,
    pub repo: Option<String>,
    pub tags: Vec<String>,
    /// Only the official catalog annotates the category.
    pub category: String,
    /// Only the official catalog sets this; search results use [`HubSources::installed`].
    pub installed: bool,
}

/// Hub lock entry: how an installed skill was obtained and scanned.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct HubInstalled {
    pub name: Option<String>,
    pub trust_level: Option<String>,
    pub scan_verdict: Option<String>,
}

/// `identifier -> lock entry` map returned by the sources and search routes.
pub type HubInstalledMap = BTreeMap<String, HubInstalled>;

/// `GET /api/skills/hub/official`: the entire optional-skills catalog, local scan.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct OfficialSkills {
    pub skills: Vec<HubSkill>,
}

/// A configured hub source row (GitHub, hermes-index, …).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct HubSource {
    pub id: String,
    pub label: String,
    pub rate_limited: Option<bool>,
    pub available: Option<bool>,
    pub searchable: Option<bool>,
}

/// `GET /api/skills/hub/sources`: sources + featured skills. Network-bound.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct HubSources {
    pub sources: Vec<HubSource>,
    pub index_available: bool,
    pub featured: Vec<HubSkill>,
    pub installed: HubInstalledMap,
}

/// `GET /api/skills/hub/search`: deduped results across sources. Network-bound.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct HubSearch {
    pub results: Vec<HubSkill>,
    /// Source id → number of hits (free-form; older hubs may omit it).
    pub source_counts: Value,
    pub timed_out: Vec<String>,
    pub installed: HubInstalledMap,
}

/// `GET /api/skills/hub/preview`: SKILL.md + manifest without installing.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SkillPreview {
    pub name: String,
    pub description: String,
    pub source: String,
    pub identifier: String,
    pub trust_level: String,
    pub repo: Option<String>,
    pub tags: Vec<String>,
    pub skill_md: String,
    pub files: Vec<String>,
}

/// One finding of the install-time security scan.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ScanFinding {
    pub severity: String,
    pub category: String,
    pub file: Option<String>,
    pub line: Option<i64>,
    pub description: String,
}

/// `GET /api/skills/hub/scan`: security verdict before installing. Network-bound.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SkillScan {
    pub name: String,
    pub identifier: String,
    pub source: String,
    pub trust_level: String,
    pub verdict: String,
    pub summary: String,
    /// `allow` | `ask` | `block`.
    pub policy: String,
    pub policy_reason: Option<String>,
    pub findings: Vec<ScanFinding>,
    pub severity_counts: BTreeMap<String, i64>,
    /// Advisory Tier-1 scanner output; `null` when the optional scanner is absent.
    pub tier1: Value,
}

/// Reply of any spawn-an-action route (`{ok, pid, name}`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SpawnAction {
    pub ok: bool,
    pub pid: Option<i64>,
    pub name: String,
}

/// `GET /api/skills/hub/official?profile=`: local catalog scan (no network).
pub fn skills_hub_official_request(profile: &str) -> Request {
    req(
        Method::Get,
        format!("/api/skills/hub/official{}", profile_query(profile)),
        None,
    )
}

/// Local scan of the official optional-skills catalog.
pub async fn skills_hub_official(rest: &Rest, profile: &str) -> Result<OfficialSkills, ApiError> {
    send_as(rest, &skills_hub_official_request(profile)).await
}

/// `GET /api/skills/hub/sources?profile=`: configured sources + featured skills.
pub fn skills_hub_sources_request(profile: &str) -> Request {
    req(
        Method::Get,
        format!("/api/skills/hub/sources{}", profile_query(profile)),
        None,
    )
}

/// Configured hub sources and featured skills. **Network** (index + GitHub).
pub async fn skills_hub_sources(rest: &Rest, profile: &str) -> Result<HubSources, ApiError> {
    send_as(rest, &skills_hub_sources_request(profile)).await
}

/// `GET /api/skills/hub/search?q=&source=&limit=`: capped at 50 results.
pub fn skills_hub_search_request(q: &str, source: &str, limit: u32, profile: &str) -> Request {
    let capped = if limit == 0 {
        String::new()
    } else {
        limit.min(50).to_string()
    };
    let path = with_query(
        "/api/skills/hub/search",
        &[
            ("q", q),
            ("source", source),
            ("limit", capped.as_str()),
            ("profile", profile),
        ],
    );
    req(Method::Get, path, None)
}

/// Search every configured hub source. **Network**; empty `q` returns no results.
pub async fn skills_hub_search(
    rest: &Rest,
    q: &str,
    source: &str,
    limit: u32,
    profile: &str,
) -> Result<HubSearch, ApiError> {
    send_as(rest, &skills_hub_search_request(q, source, limit, profile)).await
}

/// `GET /api/skills/hub/preview?identifier=`: SKILL.md + file list, no install.
pub fn skills_hub_preview_request(identifier: &str, profile: &str) -> Request {
    let path = with_query(
        "/api/skills/hub/preview",
        &[("identifier", identifier), ("profile", profile)],
    );
    req(Method::Get, path, None)
}

/// Fetch a hub skill's SKILL.md without installing. **Network**.
pub async fn skills_hub_preview(rest: &Rest, identifier: &str, profile: &str) -> Result<SkillPreview, ApiError> {
    send_as(rest, &skills_hub_preview_request(identifier, profile)).await
}

/// `GET /api/skills/hub/scan?identifier=`: install-time security scan.
pub fn skills_hub_scan_request(identifier: &str, profile: &str) -> Request {
    let path = with_query(
        "/api/skills/hub/scan",
        &[("identifier", identifier), ("profile", profile)],
    );
    req(Method::Get, path, None)
}

/// Scan a hub skill before installing. **Network**; can take tens of seconds.
pub async fn skills_hub_scan(rest: &Rest, identifier: &str, profile: &str) -> Result<SkillScan, ApiError> {
    send_as(rest, &skills_hub_scan_request(identifier, profile)).await
}

/// `POST /api/skills/hub/install`: spawns `hermes skills install <id> --yes`.
pub fn skills_hub_install_request(identifier: &str, profile: &str) -> Request {
    req(
        Method::Post,
        format!("/api/skills/hub/install{}", profile_query(profile)),
        Some(json!({ "identifier": identifier })),
    )
}

/// Install a hub skill in the background. **Spawns a process / network**.
pub async fn skills_hub_install(rest: &Rest, identifier: &str, profile: &str) -> Result<SpawnAction, ApiError> {
    send_as(rest, &skills_hub_install_request(identifier, profile)).await
}

/// `POST /api/skills/hub/uninstall`: spawns `hermes skills uninstall <name> --yes`.
pub fn skills_hub_uninstall_request(name: &str, profile: &str) -> Request {
    req(
        Method::Post,
        format!("/api/skills/hub/uninstall{}", profile_query(profile)),
        Some(json!({ "name": name })),
    )
}

/// Uninstall a hub skill in the background. **Spawns a process / destructive**.
pub async fn skills_hub_uninstall(rest: &Rest, name: &str, profile: &str) -> Result<SpawnAction, ApiError> {
    send_as(rest, &skills_hub_uninstall_request(name, profile)).await
}

/// `POST /api/skills/hub/update`: spawns `hermes skills update`.
pub fn skills_hub_update_request(profile: &str) -> Request {
    req(
        Method::Post,
        format!("/api/skills/hub/update{}", profile_query(profile)),
        None,
    )
}

/// Update every hub-installed skill. **Spawns a process / network**.
pub async fn skills_hub_update(rest: &Rest, profile: &str) -> Result<SpawnAction, ApiError> {
    send_as(rest, &skills_hub_update_request(profile)).await
}

// ── Skills RPCs ────────────────────────────────────────────────────────────────

/// `skills.manage` with `action` = `list` | `search` | `browse` | `inspect` | `install`.
pub fn skills_manage_call(action: &str, query: &str, profile: &str) -> Call {
    let mut params = json!({ "action": action, "query": query });
    put_str(&mut params, "profile", profile);
    call_of("skills.manage", params)
}

/// Run one skills.manage action; the reply shape depends on the action (`{skills}`,
/// `{results,…}`, `{installed,name}`, `{info}`). `install` **spawns/network**.
pub async fn skills_manage(client: &Client, action: &str, query: &str, profile: &str) -> Result<Value, ApiError> {
    call(client, &skills_manage_call(action, query, profile)).await
}

/// One skill touched by a registry rescan.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SkillReloadEntry {
    pub name: String,
}

/// Payload of a skills registry rescan.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SkillsReloadResult {
    pub added: Vec<SkillReloadEntry>,
    pub removed: Vec<SkillReloadEntry>,
    pub total: i64,
}

/// `skills.reload`: human-readable log + machine counts.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SkillsReload {
    pub output: String,
    pub result: SkillsReloadResult,
}

/// `skills.reload`: rescan the skills directory of the session's profile.
pub fn skills_reload_call(session_id: &str, profile: &str) -> Call {
    let mut params = json!({});
    put_str(&mut params, "session_id", session_id);
    put_str(&mut params, "profile", profile);
    call_of("skills.reload", params)
}

/// Rescan skills on disk. Local only, no network.
pub async fn skills_reload(client: &Client, session_id: &str, profile: &str) -> Result<SkillsReload, ApiError> {
    call_as(client, &skills_reload_call(session_id, profile)).await
}

// ── Tools / toolsets (REST config panels) ─────────────────────────────────────

/// One env var a provider needs, with its configured state (values never sent).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ProviderEnvVar {
    pub key: String,
    pub prompt: String,
    pub url: Option<String>,
    pub default: Option<String>,
    pub is_set: bool,
}

/// One provider row of a toolset config panel.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ToolsetProvider {
    pub name: String,
    pub badge: String,
    pub tag: String,
    pub env_vars: Vec<ProviderEnvVar>,
    /// Post-setup action key, when the provider needs one.
    pub post_setup: Option<String>,
    pub requires_nous_auth: bool,
    pub managed_nous_feature: Option<String>,
    pub is_active: bool,
    /// `ready` | `needs_key` | `needs_setup` | `unavailable` | …
    pub status: String,
    /// Web only: backend key for per-capability selection.
    pub web_backend: Option<String>,
    /// Web only: `{search, extract}` capability flags.
    pub capabilities: Value,
    /// TTS only: key written to `tts.provider`.
    pub tts_provider: Option<String>,
}

/// `GET /api/tools/toolsets/{name}/config`: provider matrix + key status.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ToolsetConfig {
    pub name: String,
    pub has_category: bool,
    pub providers: Vec<ToolsetProvider>,
    pub active_provider: Option<String>,
    pub active_search_backend: Option<String>,
    pub active_extract_backend: Option<String>,
    pub search_via_nous: bool,
    pub extract_via_nous: bool,
}

/// One model of a toolset's model catalog (image/video generation).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ToolsetModel {
    pub id: String,
    pub display: String,
    pub speed: String,
    pub strengths: String,
    pub price: String,
}

/// `GET /api/tools/toolsets/{name}/models`: model catalog for a backend.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ToolsetModels {
    pub name: String,
    pub has_models: bool,
    pub provider: Option<String>,
    pub plugin: Option<String>,
    pub models: Vec<ToolsetModel>,
    pub current: Option<String>,
    pub default: Option<String>,
}

/// `GET /api/tools/toolsets/{name}/config?profile=`. A toolset with a provider
/// category contacts the Nous portal (`force_fresh`) — treat as **network**.
pub fn toolset_config_request(name: &str, profile: &str) -> Request {
    req(
        Method::Get,
        format!("/api/tools/toolsets/{}/config{}", seg(name), profile_query(profile)),
        None,
    )
}

/// Provider matrix + env-var key status for one toolset.
pub async fn toolset_config(rest: &Rest, name: &str, profile: &str) -> Result<ToolsetConfig, ApiError> {
    send_as(rest, &toolset_config_request(name, profile)).await
}

/// `GET /api/tools/toolsets/{name}/models?provider=`: model catalog (image/video).
pub fn toolset_models_request(name: &str, provider: &str, profile: &str) -> Request {
    let path = with_query(
        &format!("/api/tools/toolsets/{}/models", seg(name)),
        &[("provider", provider), ("profile", profile)],
    );
    req(Method::Get, path, None)
}

/// Model catalog for a toolset backend. **Network** (entitlement probe).
pub async fn toolset_models(rest: &Rest, name: &str, provider: &str, profile: &str) -> Result<ToolsetModels, ApiError> {
    send_as(rest, &toolset_models_request(name, provider, profile)).await
}

/// `PUT /api/tools/toolsets/{name}/model`: persist the selected model.
pub fn toolset_select_model_request(name: &str, model: &str, provider: &str, profile: &str) -> Request {
    let mut body = json!({ "model": model });
    put_str(&mut body, "provider", provider);
    req(
        Method::Put,
        format!("/api/tools/toolsets/{}/model{}", seg(name), profile_query(profile)),
        Some(body_with_profile(body, profile)),
    )
}

/// Persist a toolset's model. **Spawns/network** on some backends (catalog refresh).
pub async fn toolset_select_model(
    rest: &Rest,
    name: &str,
    model: &str,
    provider: &str,
    profile: &str,
) -> Result<Value, ApiError> {
    send_as(rest, &toolset_select_model_request(name, model, provider, profile)).await
}

/// `PUT /api/tools/toolsets/{name}/provider`: persist the active provider.
pub fn toolset_select_provider_request(name: &str, provider: &str, capability: &str, profile: &str) -> Request {
    let mut body = json!({ "provider": provider });
    put_str(&mut body, "capability", capability); // web only: `search` | `extract`
    req(
        Method::Put,
        format!("/api/tools/toolsets/{}/provider{}", seg(name), profile_query(profile)),
        Some(body_with_profile(body, profile)),
    )
}

/// Persist a toolset's active provider (optionally per web capability).
pub async fn toolset_select_provider(
    rest: &Rest,
    name: &str,
    provider: &str,
    capability: &str,
    profile: &str,
) -> Result<Value, ApiError> {
    send_as(
        rest,
        &toolset_select_provider_request(name, provider, capability, profile),
    )
    .await
}

/// `PUT /api/tools/toolsets/{name}/env`: write provider credentials.
pub fn toolset_save_env_request(name: &str, env: Value, profile: &str) -> Request {
    req(
        Method::Put,
        format!("/api/tools/toolsets/{}/env{}", seg(name), profile_query(profile)),
        Some(body_with_profile(json!({ "env": env }), profile)),
    )
}

/// Write provider env vars. **Reveals secrets** (credentials in the body).
pub async fn toolset_save_env(rest: &Rest, name: &str, env: Value, profile: &str) -> Result<Value, ApiError> {
    send_as(rest, &toolset_save_env_request(name, env, profile)).await
}

/// `POST /api/tools/toolsets/{name}/post-setup`: run one install step.
pub fn toolset_post_setup_request(name: &str, key: &str, profile: &str) -> Request {
    req(
        Method::Post,
        format!("/api/tools/toolsets/{}/post-setup{}", seg(name), profile_query(profile)),
        Some(body_with_profile(json!({ "key": key }), profile)),
    )
}

/// Run a provider post-setup install. **Spawns a process** (pip/apt/…).
pub async fn toolset_post_setup(rest: &Rest, name: &str, key: &str, profile: &str) -> Result<Value, ApiError> {
    send_as(rest, &toolset_post_setup_request(name, key, profile)).await
}

// ── Tools / toolsets RPCs ─────────────────────────────────────────────────────

/// One row of `tools.list` / `toolsets.list`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ToolsetRow {
    pub name: String,
    pub description: String,
    pub tool_count: i64,
    pub enabled: bool,
    /// Tool names; only `tools.list` fills this.
    pub tools: Vec<String>,
}

/// `tools.list` / `toolsets.list` reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct Toolsets {
    pub toolsets: Vec<ToolsetRow>,
}

/// One tool definition inside `tools.show`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
}

/// One toolset section of `tools.show`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ToolSection {
    pub name: String,
    pub tools: Vec<ToolDefinition>,
}

/// `tools.show`: every tool the current toolset selection exposes, grouped.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ToolsShow {
    pub sections: Vec<ToolSection>,
    pub total: i64,
}

/// `tools.configure` reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ToolsConfigure {
    pub changed: Vec<String>,
    pub enabled_toolsets: Vec<String>,
    /// Agent reset info when a live session was reconfigured.
    pub info: Value,
    pub missing_servers: Vec<String>,
    pub reset: bool,
    pub unknown: Vec<String>,
}

/// `tools.list`: toolsets with their resolved tool names.
pub fn tools_list_call(session_id: &str) -> Call {
    let mut params = json!({});
    put_str(&mut params, "session_id", session_id);
    call_of("tools.list", params)
}

/// Toolsets plus resolved tool names for the calling session.
pub async fn tools_list(client: &Client, session_id: &str) -> Result<Toolsets, ApiError> {
    call_as(client, &tools_list_call(session_id)).await
}

/// `toolsets.list`: toolsets only (no tool names).
pub fn toolsets_list_call(session_id: &str) -> Call {
    let mut params = json!({});
    put_str(&mut params, "session_id", session_id);
    call_of("toolsets.list", params)
}

/// Toolset rows without the per-tool list.
pub async fn toolsets_list(client: &Client, session_id: &str) -> Result<Toolsets, ApiError> {
    call_as(client, &toolsets_list_call(session_id)).await
}

/// `tools.show`: every available tool grouped by toolset, with a total.
pub fn tools_show_call(session_id: &str) -> Call {
    let mut params = json!({});
    put_str(&mut params, "session_id", session_id);
    call_of("tools.show", params)
}

/// Full tool listing for the `/tools` screen.
pub async fn tools_show(client: &Client, session_id: &str) -> Result<ToolsShow, ApiError> {
    call_as(client, &tools_show_call(session_id)).await
}

/// `tools.configure`: enable/disable toolsets or `server:tool` targets.
pub fn tools_configure_call(action: &str, names: &[&str], session_id: &str) -> Call {
    let mut params = json!({ "action": action, "names": names });
    put_str(&mut params, "session_id", session_id);
    call_of("tools.configure", params)
}

/// Enable or disable toolsets/MCP tools. **Writes config** and may reset a live
/// session's agent (`reset: true`).
pub async fn tools_configure(
    client: &Client,
    action: &str,
    names: &[&str],
    session_id: &str,
) -> Result<ToolsConfigure, ApiError> {
    call_as(client, &tools_configure_call(action, names, session_id)).await
}

// ── Terminal backends ─────────────────────────────────────────────────────────

/// One terminal backend row with its probed health.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct TerminalBackend {
    pub name: String,
    pub label: String,
    pub description: String,
    pub active: bool,
    /// `ready` | `needs_setup` | `unavailable`.
    pub status: String,
    pub detail: Option<String>,
}

/// `GET /api/tools/terminal/backends`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct TerminalBackends {
    pub active: String,
    pub backends: Vec<TerminalBackend>,
}

/// `PUT /api/tools/terminal/backend` reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct TerminalBackendSelected {
    pub ok: bool,
    pub backend: String,
}

/// `GET /api/tools/terminal/backends?profile=`: probes each backend (local commands).
pub fn terminal_backends_request(profile: &str) -> Request {
    req(
        Method::Get,
        format!("/api/tools/terminal/backends{}", profile_query(profile)),
        None,
    )
}

/// Terminal backend rows and their health. **Spawns probes** (docker/ssh/…).
pub async fn terminal_backends(rest: &Rest, profile: &str) -> Result<TerminalBackends, ApiError> {
    send_as(rest, &terminal_backends_request(profile)).await
}

/// `PUT /api/tools/terminal/backend`: persist `terminal.backend`.
pub fn select_terminal_backend_request(backend: &str, profile: &str) -> Request {
    req(
        Method::Put,
        format!("/api/tools/terminal/backend{}", profile_query(profile)),
        Some(body_with_profile(json!({ "backend": backend }), profile)),
    )
}

/// Choose the terminal backend. A backend that still needs setup is allowed.
pub async fn select_terminal_backend(
    rest: &Rest,
    backend: &str,
    profile: &str,
) -> Result<TerminalBackendSelected, ApiError> {
    send_as(rest, &select_terminal_backend_request(backend, profile)).await
}

// ── Computer use ──────────────────────────────────────────────────────────────

/// `GET /api/tools/computer-use/status`: readiness card.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ComputerUseStatus {
    pub platform: String,
    pub ready: bool,
    pub can_grant: bool,
    /// Per-permission checks (shape owned by `tools.computer_use.permissions`).
    pub checks: Value,
}

/// `GET /api/tools/computer-use/status?profile=`.
pub fn computer_use_status_request(profile: &str) -> Request {
    req(
        Method::Get,
        format!("/api/tools/computer-use/status{}", profile_query(profile)),
        None,
    )
}

/// Computer-use readiness for the desktop card.
pub async fn computer_use_status(rest: &Rest, profile: &str) -> Result<ComputerUseStatus, ApiError> {
    send_as(rest, &computer_use_status_request(profile)).await
}

/// `POST /api/tools/computer-use/permissions/grant` (macOS only).
pub fn grant_computer_use_permissions_request(profile: &str) -> Request {
    req(
        Method::Post,
        format!("/api/tools/computer-use/permissions/grant{}", profile_query(profile)),
        None,
    )
}

/// Request screen/accessibility permissions. **Spawns a process**; macOS only.
pub async fn grant_computer_use_permissions(rest: &Rest, profile: &str) -> Result<SpawnAction, ApiError> {
    send_as(rest, &grant_computer_use_permissions_request(profile)).await
}

// ── Browser (CDP) control ─────────────────────────────────────────────────────

/// `browser.manage` reply. `messages` only accompanies a `connect` that had progress to
/// report; `browser_use` is only set by `status` / `use`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct BrowserManaged {
    pub connected: bool,
    pub url: Option<String>,
    pub browser_use: bool,
    pub messages: Vec<String>,
}

/// `browser.manage`: one method for the whole browser card. `action` is
/// `status` | `use` | `connect` | `disconnect`; `enabled` is the `use` argument, `url` the
/// optional CDP endpoint for `connect` (empty = the backend's `http://127.0.0.1:9222`).
pub fn browser_manage_call(action: &str, enabled: Option<bool>, url: &str, session_id: &str, profile: &str) -> Call {
    let mut params = json!({ "action": action });
    if let Some(v) = enabled {
        params["enabled"] = json!(v);
    }
    put_str(&mut params, "url", url);
    put_str(&mut params, "session_id", session_id);
    put_str(&mut params, "profile", profile);
    call_of("browser.manage", params)
}

/// `browser.manage {action: "status"}`: the current CDP override and browser-use mode.
/// Read-only (`_resolve_browser_cdp_url` reads env/config).
pub async fn browser_status(client: &Client, profile: &str) -> Result<BrowserManaged, ApiError> {
    call_as(client, &browser_manage_call("status", None, "", "", profile)).await
}

/// `browser.manage {action: "use"}`: switch the backend's browser-use CLI mode.
///
/// **Writes backend state** (a process-wide mode flag); builder + unit test only.
pub async fn browser_use(client: &Client, enabled: bool, profile: &str) -> Result<BrowserManaged, ApiError> {
    call_as(client, &browser_manage_call("use", Some(enabled), "", "", profile)).await
}

/// `browser.manage {action: "connect"}`: point the backend at a CDP endpoint (reaps and
/// relaunches browser supervisors).
///
/// **Spawns processes / writes backend state**; builder + unit test only.
pub async fn browser_connect(
    client: &Client,
    url: &str,
    session_id: &str,
    profile: &str,
) -> Result<BrowserManaged, ApiError> {
    call_as(client, &browser_manage_call("connect", None, url, session_id, profile)).await
}

/// `browser.manage {action: "disconnect"}`: drop the CDP override and reap the supervisors.
///
/// **Spawns processes / writes backend state**; builder + unit test only.
pub async fn browser_disconnect(client: &Client, profile: &str) -> Result<BrowserManaged, ApiError> {
    call_as(client, &browser_manage_call("disconnect", None, "", "", profile)).await
}

// ── MCP: REST catalog + whole-map replace ─────────────────────────────────────

/// One env var a catalog entry declares (never the value).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct McpEnvRequirement {
    pub name: String,
    pub prompt: String,
    pub required: bool,
}

/// Composer-suggestion triggers declared by a catalog entry.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct McpSuggest {
    pub keywords: Vec<String>,
    pub hosts: Vec<String>,
    pub applications: Vec<String>,
    pub examples: Vec<String>,
    pub requires_app: bool,
}

/// One entry of `GET /api/mcp/catalog`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct McpCatalogEntry {
    pub name: String,
    pub description: String,
    pub connector_slug: Option<String>,
    pub source: String,
    /// `stdio` | `http`.
    pub transport: String,
    /// `none` | `env` | `oauth` | …
    pub auth_type: String,
    pub required_env: Vec<McpEnvRequirement>,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub url: Option<String>,
    pub install_url: Option<String>,
    pub install_ref: Option<String>,
    pub bootstrap: Vec<String>,
    pub default_enabled: Option<Vec<String>>,
    pub post_install: String,
    pub suggest: Option<McpSuggest>,
    /// True when installing requires a git clone/build.
    pub needs_install: bool,
    pub installed: bool,
    pub enabled: bool,
}

/// A catalog-manifest diagnostic row.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct McpCatalogDiagnostic {
    pub name: String,
    pub kind: String,
    pub message: String,
}

/// `GET /api/mcp/catalog`: curated presets with per-profile state.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct McpCatalog {
    pub entries: Vec<McpCatalogEntry>,
    pub diagnostics: Vec<McpCatalogDiagnostic>,
}

/// `GET /api/mcp/catalog?profile=`: local manifests, no network.
pub fn mcp_catalog_request(profile: &str) -> Request {
    req(Method::Get, format!("/api/mcp/catalog{}", profile_query(profile)), None)
}

/// Curated MCP presets annotated with installed/enabled state.
pub async fn mcp_catalog(rest: &Rest, profile: &str) -> Result<McpCatalog, ApiError> {
    send_as(rest, &mcp_catalog_request(profile)).await
}

/// `POST /api/mcp/catalog/install`: install a preset into `config.yaml`.
pub fn mcp_catalog_install_request(name: &str, env: Value, enable: bool, profile: &str) -> Request {
    req(
        Method::Post,
        format!("/api/mcp/catalog/install{}", profile_query(profile)),
        Some(body_with_profile(
            json!({ "name": name, "env": env, "enable": enable }),
            profile,
        )),
    )
}

/// Install a catalog entry. **Spawns a process / network**; writes `.env` secrets.
pub async fn mcp_catalog_install(
    rest: &Rest,
    name: &str,
    env: Value,
    enable: bool,
    profile: &str,
) -> Result<Value, ApiError> {
    send_as(rest, &mcp_catalog_install_request(name, env, enable, profile)).await
}

/// `PUT /api/mcp/servers`: replace the whole `mcp_servers` map.
pub fn mcp_servers_replace_request(servers: Value, profile: &str) -> Request {
    req(
        Method::Put,
        format!("/api/mcp/servers{}", profile_query(profile)),
        Some(body_with_profile(json!({ "servers": servers }), profile)),
    )
}

/// Replace the entire MCP server map (mcp.json editor save). **Destructive**:
/// servers omitted from `servers` are deleted.
pub async fn mcp_servers_replace(rest: &Rest, servers: Value, profile: &str) -> Result<Value, ApiError> {
    send_as(rest, &mcp_servers_replace_request(servers, profile)).await
}

/// One configured MCP server from `GET /api/mcp/servers`. Secrets are reduced
/// to redacted previews; `tools: null` means every tool the server exposes.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct McpServerConfigEntry {
    pub name: String,
    /// `http` | `stdio` | `unknown`.
    pub transport: String,
    pub url: Option<String>,
    pub command: Option<String>,
    pub args: Vec<String>,
    /// Redacted env map (values are previews, never the secret).
    pub env: Value,
    pub auth: Option<String>,
    pub enabled: bool,
    pub tools: Option<Vec<String>>,
    /// `config` or `plugin`.
    pub source: String,
    pub plugin: Option<String>,
}

/// `GET /api/mcp/servers` reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct McpServerConfigs {
    pub servers: Vec<McpServerConfigEntry>,
}

pub fn mcp_servers_request(profile: &str) -> Request {
    req(Method::Get, format!("/api/mcp/servers{}", profile_query(profile)), None)
}

/// List the profile's configured MCP servers (read-only; env values redacted).
pub async fn mcp_servers(rest: &Rest, profile: &str) -> Result<McpServerConfigs, ApiError> {
    send_as(rest, &mcp_servers_request(profile)).await
}

// ── MCP: RPCs ─────────────────────────────────────────────────────────────────

/// One configured MCP server; secrets are reduced to env-var key names.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct McpServerSummary {
    pub name: String,
    /// `stdio` | `http` | `unknown`.
    pub transport: String,
    pub url: Option<String>,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env: Vec<String>,
    pub auth: Option<String>,
    pub oauth_tokens_present: Option<bool>,
    pub enabled: bool,
    pub tools: Value,
    /// `config` | `plugin`.
    pub source: String,
    pub plugin: Option<String>,
}

/// `mcp.servers.list` reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct McpServersList {
    pub servers: Vec<McpServerSummary>,
}

/// One cached runtime-state row of `mcp.servers.status`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct McpServerRuntime {
    pub name: String,
    pub transport: String,
    pub tools: i64,
    pub connected: bool,
    pub disabled: bool,
    /// `connected` | `disabled` | `connecting` | `failed` | `lazy` | `configured`.
    pub status: String,
    pub source: String,
    pub plugin: Option<String>,
}

/// `mcp.servers.status`: cached state + a millisecond timestamp.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct McpServersStatus {
    pub servers: Vec<McpServerRuntime>,
    pub checked_at: i64,
}

/// `mcp.servers.add` reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct McpServerAdded {
    pub ok: bool,
    pub name: String,
    pub server: McpServerSummary,
}

/// `mcp.servers.remove` reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct McpServerRemoved {
    pub ok: bool,
    pub removed: bool,
}

/// One tool returned by an MCP probe.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct McpProbeTool {
    pub name: String,
    pub description: String,
}

/// `mcp.servers.test`: connect, list tools, disconnect.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct McpProbe {
    pub ok: bool,
    pub error: Option<String>,
    pub tools: Vec<McpProbeTool>,
    pub prompts: i64,
    pub resources: i64,
    pub oauth_needed: bool,
    pub oauth_tokens_present: Option<bool>,
}

/// `mcp.servers.set_api_key` reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct McpApiKeySet {
    pub ok: bool,
    pub name: String,
    pub env_var: String,
    pub server: McpServerSummary,
}

/// `mcp.servers.oauth.start` reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct McpOAuthStart {
    pub ok: bool,
    pub session_id: String,
    pub auth_url: String,
    pub flow: String,
}

/// `mcp.servers.oauth.poll` reply (`pending` | `approved` | `error`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct McpOAuthPoll {
    pub ok: bool,
    pub status: String,
}

/// One catalog entry of the `mcp.catalog` RPC (per-profile shape).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct McpRpcCatalogEntry {
    pub name: String,
    pub description: String,
    pub connector_slug: Option<String>,
    pub installed: bool,
    pub enabled: bool,
    /// Required env-var names.
    pub requires: Vec<String>,
    pub transport: String,
}

/// `mcp.catalog` RPC reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct McpRpcCatalog {
    pub servers: Vec<McpRpcCatalogEntry>,
}

/// `mcp.servers.list`: configured servers for the scoped profile.
pub fn mcp_servers_list_call(profile: &str) -> Call {
    let mut params = json!({});
    put_str(&mut params, "profile", profile);
    call_of("mcp.servers.list", params)
}

/// Configured MCP servers, secrets redacted to key names.
pub async fn mcp_servers_list(client: &Client, profile: &str) -> Result<McpServersList, ApiError> {
    call_as(client, &mcp_servers_list_call(profile)).await
}

/// `mcp.servers.status`: cached runtime state; never connects or probes.
pub fn mcp_servers_status_call(profile: &str) -> Call {
    let mut params = json!({});
    put_str(&mut params, "profile", profile);
    call_of("mcp.servers.status", params)
}

/// Cached connection status per configured server.
pub async fn mcp_servers_status(client: &Client, profile: &str) -> Result<McpServersStatus, ApiError> {
    call_as(client, &mcp_servers_status_call(profile)).await
}

/// `mcp.servers.add`: add from a catalog preset and/or an explicit config.
pub fn mcp_servers_add_call(name: &str, preset: &str, config: Option<Value>, profile: &str) -> Call {
    let mut params = json!({ "name": name });
    put_str(&mut params, "preset", preset);
    if let Some(config) = config {
        params["config"] = config;
    }
    put_str(&mut params, "profile", profile);
    call_of("mcp.servers.add", params)
}

/// Add an MCP server. **Writes config**; a bad command is rejected by the backend.
pub async fn mcp_servers_add(
    client: &Client,
    name: &str,
    preset: &str,
    config: Option<Value>,
    profile: &str,
) -> Result<McpServerAdded, ApiError> {
    call_as(client, &mcp_servers_add_call(name, preset, config, profile)).await
}

/// `mcp.servers.remove`: drop a server from the profile's config.
pub fn mcp_servers_remove_call(name: &str, profile: &str) -> Call {
    let mut params = json!({ "name": name });
    put_str(&mut params, "profile", profile);
    call_of("mcp.servers.remove", params)
}

/// Remove an MCP server. **Destructive** (config entry deleted).
pub async fn mcp_servers_remove(client: &Client, name: &str, profile: &str) -> Result<McpServerRemoved, ApiError> {
    call_as(client, &mcp_servers_remove_call(name, profile)).await
}

/// `mcp.servers.set_api_key`: store a credential in `.env`, reference it from config.
pub fn mcp_servers_set_api_key_call(name: &str, value: &str, env_var: &str, profile: &str) -> Call {
    let mut params = json!({ "name": name, "value": value });
    put_str(&mut params, "env_var", env_var);
    put_str(&mut params, "profile", profile);
    call_of("mcp.servers.set_api_key", params)
}

/// Store an MCP credential. **Reveals secrets** (the value crosses the wire).
pub async fn mcp_servers_set_api_key(
    client: &Client,
    name: &str,
    value: &str,
    env_var: &str,
    profile: &str,
) -> Result<McpApiKeySet, ApiError> {
    call_as(client, &mcp_servers_set_api_key_call(name, value, env_var, profile)).await
}

/// `mcp.servers.test`: connect for real and list tools.
pub fn mcp_servers_test_call(name: &str, profile: &str) -> Call {
    let mut params = json!({ "name": name });
    put_str(&mut params, "profile", profile);
    call_of("mcp.servers.test", params)
}

/// Probe one MCP server. **Spawns a process / network**; cold `npx` can block.
pub async fn mcp_servers_test(client: &Client, name: &str, profile: &str) -> Result<McpProbe, ApiError> {
    call_as(client, &mcp_servers_test_call(name, profile)).await
}

/// `mcp.servers.oauth.start`: begin a session-backed OAuth flow.
pub fn mcp_servers_oauth_start_call(name: &str, client_redirect_uri: &str, profile: &str) -> Call {
    let mut params = json!({ "name": name });
    put_str(&mut params, "client_redirect_uri", client_redirect_uri);
    put_str(&mut params, "profile", profile);
    call_of("mcp.servers.oauth.start", params)
}

/// Start an MCP OAuth flow. **Network**; the caller opens `auth_url`.
pub async fn mcp_servers_oauth_start(
    client: &Client,
    name: &str,
    client_redirect_uri: &str,
    profile: &str,
) -> Result<McpOAuthStart, ApiError> {
    call_as(
        client,
        &mcp_servers_oauth_start_call(name, client_redirect_uri, profile),
    )
    .await
}

/// `mcp.servers.oauth.poll`: poll a running flow.
pub fn mcp_servers_oauth_poll_call(name: &str, session_id: &str, profile: &str) -> Call {
    let mut params = json!({ "name": name, "session_id": session_id });
    put_str(&mut params, "profile", profile);
    call_of("mcp.servers.oauth.poll", params)
}

/// Poll an MCP OAuth flow. **Network**.
pub async fn mcp_servers_oauth_poll(
    client: &Client,
    name: &str,
    session_id: &str,
    profile: &str,
) -> Result<McpOAuthPoll, ApiError> {
    call_as(client, &mcp_servers_oauth_poll_call(name, session_id, profile)).await
}

/// `mcp.servers.oauth.cancel`: abandon a flow.
pub fn mcp_servers_oauth_cancel_call(name: &str, session_id: &str, profile: &str) -> Call {
    let mut params = json!({ "name": name, "session_id": session_id });
    put_str(&mut params, "profile", profile);
    call_of("mcp.servers.oauth.cancel", params)
}

/// Cancel an MCP OAuth flow.
pub async fn mcp_servers_oauth_cancel(
    client: &Client,
    name: &str,
    session_id: &str,
    profile: &str,
) -> Result<Value, ApiError> {
    call(client, &mcp_servers_oauth_cancel_call(name, session_id, profile)).await
}

/// `mcp.servers.oauth.callback`: relay a client-captured redirect.
pub fn mcp_servers_oauth_callback_call(name: &str, session_id: &str, code: &str, state: &str, profile: &str) -> Call {
    let mut params = json!({ "name": name, "session_id": session_id });
    put_str(&mut params, "code", code);
    put_str(&mut params, "state", state);
    put_str(&mut params, "profile", profile);
    call_of("mcp.servers.oauth.callback", params)
}

/// Relay a client-captured OAuth code into the flow.
pub async fn mcp_servers_oauth_callback(
    client: &Client,
    name: &str,
    session_id: &str,
    code: &str,
    state: &str,
    profile: &str,
) -> Result<Value, ApiError> {
    call(
        client,
        &mcp_servers_oauth_callback_call(name, session_id, code, state, profile),
    )
    .await
}

/// `mcp.catalog` RPC: per-profile preset state (env keys as `requires`).
pub fn mcp_catalog_rpc_call(profile: &str) -> Call {
    let mut params = json!({});
    put_str(&mut params, "profile", profile);
    call_of("mcp.catalog", params)
}

/// Curated MCP presets via the gateway. Local manifests, no network.
pub async fn mcp_catalog_rpc(client: &Client, profile: &str) -> Result<McpRpcCatalog, ApiError> {
    call_as(client, &mcp_catalog_rpc_call(profile)).await
}

// ── Learning graph (/journey) ─────────────────────────────────────────────────

/// One learned skill or memory chunk.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct LearningNode {
    pub id: String,
    pub label: String,
    pub kind: String,
    /// Millisecond epoch or ISO string, depending on the node kind.
    pub timestamp: Value,
    pub category: String,
    #[serde(rename = "useCount")]
    pub use_count: i64,
    pub state: String,
    #[serde(rename = "createdBy")]
    pub created_by: Option<String>,
    pub pinned: bool,
    #[serde(rename = "memorySource")]
    pub memory_source: Option<String>,
}

/// One edge between learning nodes.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct LearningEdge {
    pub source: String,
    pub target: String,
}

/// `GET /api/learning/graph`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct LearningGraph {
    pub nodes: Vec<LearningNode>,
    pub edges: Vec<LearningEdge>,
    /// Category clusters (free-form row list owned by the renderer).
    pub clusters: Value,
    pub memory: Value,
    pub stats: Value,
}

/// `GET /api/learning/node` / `PUT` / `DELETE` reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct LearningDetail {
    pub ok: bool,
    pub message: Option<String>,
    pub kind: Option<String>,
    pub id: Option<String>,
    pub label: Option<String>,
    pub content: Option<String>,
}

/// Mutation reply of `PUT`/`DELETE /api/learning/node`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct LearningMutation {
    pub ok: bool,
    pub message: Option<String>,
}

/// `GET /api/learning/graph?profile=`.
pub fn learning_graph_request(profile: &str) -> Request {
    req(
        Method::Get,
        format!("/api/learning/graph{}", profile_query(profile)),
        None,
    )
}

/// Profile-scoped learning graph (learned skills + memory chunks).
pub async fn learning_graph(rest: &Rest, profile: &str) -> Result<LearningGraph, ApiError> {
    send_as(rest, &learning_graph_request(profile)).await
}

/// `GET /api/learning/node?id=`: node content for an edit prefill.
pub fn learning_node_request(id: &str, profile: &str) -> Request {
    let path = with_query("/api/learning/node", &[("id", id), ("profile", profile)]);
    req(Method::Get, path, None)
}

/// Fetch one node's content (404 for an unknown id).
pub async fn learning_node(rest: &Rest, id: &str, profile: &str) -> Result<LearningDetail, ApiError> {
    send_as(rest, &learning_node_request(id, profile)).await
}

/// `PUT /api/learning/node`: rewrite a skills/memory node.
pub fn learning_node_update_request(id: &str, content: &str, profile: &str) -> Request {
    req(
        Method::Put,
        format!("/api/learning/node{}", profile_query(profile)),
        Some(body_with_profile(json!({ "id": id, "content": content }), profile)),
    )
}

/// Rewrite a node's content. **Writes user data** (skill SKILL.md or memory).
pub async fn learning_node_update(
    rest: &Rest,
    id: &str,
    content: &str,
    profile: &str,
) -> Result<LearningMutation, ApiError> {
    send_as(rest, &learning_node_update_request(id, content, profile)).await
}

/// `DELETE /api/learning/node`: archive a skill or remove a memory chunk.
pub fn learning_node_delete_request(id: &str, profile: &str) -> Request {
    req(
        Method::Delete,
        format!("/api/learning/node{}", profile_query(profile)),
        Some(body_with_profile(json!({ "id": id }), profile)),
    )
}

/// Delete a learning node. **Destructive** (skill archived, memory removed).
pub async fn learning_node_delete(rest: &Rest, id: &str, profile: &str) -> Result<LearningMutation, ApiError> {
    send_as(rest, &learning_node_delete_request(id, profile)).await
}

/// One legend/category row of the rendered journey.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct LearningLegendItem {
    pub glyph: String,
    pub label: String,
    pub style: Option<String>,
    pub color: Option<String>,
}

/// Axis range of the rendered journey.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct LearningAxis {
    pub start: String,
    pub end: String,
}

/// `learning.frames`: pre-rendered `/journey` timeline for a TUI.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct LearningFrames {
    /// Frame projections; the grid run tuples are renderer-owned, so kept as JSON.
    pub frames: Vec<Value>,
    pub legend: Vec<LearningLegendItem>,
    pub categories: Vec<LearningLegendItem>,
    /// Bucket rows (renderer-owned shape).
    pub buckets: Vec<Value>,
    pub summary: Vec<String>,
    pub axis: Option<LearningAxis>,
    pub count: i64,
    pub cols: i64,
    pub rows: i64,
}

/// `learning.frames`: render the journey timeline server-side.
pub fn learning_frames_call(cols: i64, rows: i64, frames: i64) -> Call {
    call_of(
        "learning.frames",
        json!({ "cols": cols, "rows": rows, "frames": frames }),
    )
}

/// Pre-render the journey timeline. Local compute only.
pub async fn learning_frames(client: &Client, cols: i64, rows: i64, frames: i64) -> Result<LearningFrames, ApiError> {
    call_as(client, &learning_frames_call(cols, rows, frames)).await
}

/// `learning.detail`: node content for an edit prefill.
pub fn learning_detail_call(id: &str) -> Call {
    call_of("learning.detail", json!({ "id": id }))
}

/// Fetch one node's content over the gateway.
pub async fn learning_detail(client: &Client, id: &str) -> Result<LearningDetail, ApiError> {
    call_as(client, &learning_detail_call(id)).await
}

/// `learning.edit`: rewrite a node's content.
pub fn learning_edit_call(id: &str, content: &str) -> Call {
    call_of("learning.edit", json!({ "id": id, "content": content }))
}

/// Rewrite a skill/memory node. **Writes user data**.
pub async fn learning_edit(client: &Client, id: &str, content: &str) -> Result<LearningMutation, ApiError> {
    call_as(client, &learning_edit_call(id, content)).await
}

/// `learning.delete`: archive a skill or remove a memory chunk.
pub fn learning_delete_call(id: &str) -> Call {
    call_of("learning.delete", json!({ "id": id }))
}

/// Delete a learning node. **Destructive**.
pub async fn learning_delete(client: &Client, id: &str) -> Result<LearningMutation, ApiError> {
    call_as(client, &learning_delete_call(id)).await
}

// ── Vault ─────────────────────────────────────────────────────────────────────

/// One vault item; secrets never appear in any response.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct VaultItem {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub origin: Option<String>,
    /// Creation timestamp (string or epoch, depending on the backend).
    pub created_at: Value,
    /// Which backend served the item (`local` or a password manager).
    pub backend: String,
    pub identifier: Option<String>,
    pub identifier_type: Option<String>,
    pub has_otp: bool,
    pub allowed_origins: Vec<String>,
}

/// `vault.list`: metadata-only listing across unlocked backends.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct VaultList {
    pub items: Vec<VaultItem>,
}

/// One login source of `vault.sources`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct VaultSource {
    pub name: String,
    pub display_name: String,
    pub enabled: bool,
    pub needs_unlock: bool,
    pub unlocked: bool,
    pub installed: bool,
}

/// `vault.sources` reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct VaultSources {
    pub sources: Vec<VaultSource>,
}

/// `vault.add` reply (metadata only).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct VaultId {
    pub id: String,
}

/// `vault.remove` reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct VaultRemoved {
    pub removed: bool,
}

/// `vault.lock` reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct VaultLock {
    pub locked: bool,
}

/// `vault.unlock` reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct VaultUnlocked {
    pub name: String,
    pub unlocked: bool,
}

/// `vault.source.set` reply.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct VaultSourceState {
    pub name: String,
    pub enabled: bool,
}

/// `vault.list`: metadata for every item of every unlocked backend.
pub fn vault_list_call(profile: &str) -> Call {
    let mut params = json!({});
    put_str(&mut params, "profile", profile);
    call_of("vault.list", params)
}

/// List vault items (never passwords). Local store read.
pub async fn vault_list(client: &Client, profile: &str) -> Result<VaultList, ApiError> {
    call_as(client, &vault_list_call(profile)).await
}

/// `vault.sources`: status of the local vault and every password manager.
pub fn vault_sources_call(profile: &str) -> Call {
    let mut params = json!({});
    put_str(&mut params, "profile", profile);
    call_of("vault.sources", params)
}

/// Login-source status (installed/enabled/unlocked). Local read.
pub async fn vault_sources(client: &Client, profile: &str) -> Result<VaultSources, ApiError> {
    call_as(client, &vault_sources_call(profile)).await
}

/// `vault.add`: add a login/payment/address item.
pub fn vault_add_call(kind: &str, label: &str, origin: &str, secret: Value, profile: &str) -> Call {
    let mut params = json!({ "kind": kind, "label": label, "secret": secret });
    put_str(&mut params, "origin", origin);
    put_str(&mut params, "profile", profile);
    call_of("vault.add", params)
}

/// Add a vault item. **Reveals secrets** (the secret payload crosses the wire).
pub async fn vault_add(
    client: &Client,
    kind: &str,
    label: &str,
    origin: &str,
    secret: Value,
    profile: &str,
) -> Result<VaultId, ApiError> {
    call_as(client, &vault_add_call(kind, label, origin, secret, profile)).await
}

/// `vault.remove`: delete an item by id.
pub fn vault_remove_call(id: &str, profile: &str) -> Call {
    let mut params = json!({ "id": id });
    put_str(&mut params, "profile", profile);
    call_of("vault.remove", params)
}

/// Remove a vault item. **Destructive**.
pub async fn vault_remove(client: &Client, id: &str, profile: &str) -> Result<VaultRemoved, ApiError> {
    call_as(client, &vault_remove_call(id, profile)).await
}

/// `vault.unlock`: unlock a password manager with its master password.
pub fn vault_unlock_call(name: &str, password: &str, profile: &str) -> Call {
    let mut params = json!({ "name": name, "password": password });
    put_str(&mut params, "profile", profile);
    call_of("vault.unlock", params)
}

/// Unlock a password manager. **Reveals secrets** (master password).
pub async fn vault_unlock(
    client: &Client,
    name: &str,
    password: &str,
    profile: &str,
) -> Result<VaultUnlocked, ApiError> {
    call_as(client, &vault_unlock_call(name, password, profile)).await
}

/// `vault.lock`: forget one manager's token, or all when `name` is empty.
pub fn vault_lock_call(name: &str, profile: &str) -> Call {
    let mut params = json!({});
    put_str(&mut params, "name", name);
    put_str(&mut params, "profile", profile);
    call_of("vault.lock", params)
}

/// Lock the named password manager (all when `name` is empty).
pub async fn vault_lock(client: &Client, name: &str, profile: &str) -> Result<VaultLock, ApiError> {
    call_as(client, &vault_lock_call(name, profile)).await
}

/// `vault.source.set`: enable/disable an external password manager.
pub fn vault_source_set_call(name: &str, enabled: bool, profile: &str) -> Call {
    let mut params = json!({ "name": name, "enabled": enabled });
    put_str(&mut params, "profile", profile);
    call_of("vault.source.set", params)
}

/// Toggle a password-manager source (disabling also locks it).
pub async fn vault_source_set(
    client: &Client,
    name: &str,
    enabled: bool,
    profile: &str,
) -> Result<VaultSourceState, ApiError> {
    call_as(client, &vault_source_set_call(name, enabled, profile)).await
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn assert_get(r: &Request, path: &str) {
        assert_eq!((r.method, r.path.as_str(), &r.body), (Method::Get, path, &None));
    }

    #[test]
    fn skills_hub_builders() {
        assert_get(&skills_hub_official_request(""), "/api/skills/hub/official");
        assert_get(
            &skills_hub_official_request("swe"),
            "/api/skills/hub/official?profile=swe",
        );
        assert_get(
            &skills_hub_sources_request("swe"),
            "/api/skills/hub/sources?profile=swe",
        );
        assert_eq!(
            skills_hub_search_request("rust lang", "all", 20, "").path,
            "/api/skills/hub/search?q=rust%20lang&source=all&limit=20"
        );
        assert_eq!(
            skills_hub_search_request("x", "", 0, "swe").path,
            "/api/skills/hub/search?q=x&profile=swe"
        );
        assert_eq!(
            skills_hub_search_request("x", "", 99, "").path,
            "/api/skills/hub/search?q=x&limit=50"
        );
        assert_get(
            &skills_hub_preview_request("official/dev/x", "swe"),
            "/api/skills/hub/preview?identifier=official%2Fdev%2Fx&profile=swe",
        );
        assert_get(
            &skills_hub_scan_request("official/dev/x", ""),
            "/api/skills/hub/scan?identifier=official%2Fdev%2Fx",
        );
        assert_eq!(
            (
                skills_hub_install_request("official/dev/x", "").method,
                skills_hub_install_request("official/dev/x", "").path,
            ),
            (Method::Post, "/api/skills/hub/install".to_owned())
        );
        assert_eq!(
            skills_hub_install_request("a", "swe").body,
            Some(json!({"identifier": "a"}))
        );
        assert_eq!(
            skills_hub_uninstall_request("n", "swe").body,
            Some(json!({"name": "n"}))
        );
        assert_eq!(
            (
                skills_hub_update_request("swe").method,
                skills_hub_update_request("swe").path,
                skills_hub_update_request("swe").body,
            ),
            (Method::Post, "/api/skills/hub/update?profile=swe".to_owned(), None)
        );
    }

    #[test]
    fn skills_hub_decodes() {
        let official: OfficialSkills = crate::api::decode(json!({
            "skills": [{"name": "alpha", "description": "d", "source": "official",
                        "identifier": "official/dev/alpha", "trust_level": "builtin",
                        "repo": null, "tags": ["a"], "category": "dev", "installed": true}]
        }))
        .unwrap();
        assert_eq!(official.skills[0].identifier, "official/dev/alpha");
        assert!(official.skills[0].installed);
        assert_eq!(official.skills[0].repo, None);

        let sources: HubSources = crate::api::decode(json!({
            "sources": [{"id": "github", "label": "GitHub", "rate_limited": false, "searchable": true}],
            "index_available": true,
            "featured": [],
            "installed": {"official/dev/alpha": {"name": "alpha", "trust_level": "builtin", "scan_verdict": "pass"}}
        }))
        .unwrap();
        assert!(sources.index_available);
        assert_eq!(sources.sources[0].id, "github");
        assert_eq!(
            sources.installed["official/dev/alpha"].scan_verdict.as_deref(),
            Some("pass")
        );

        let search: HubSearch = crate::api::decode(json!({
            "results": [{"name": "x", "identifier": "i", "trust_level": "community", "tags": []}],
            "source_counts": {"github": 3},
            "timed_out": ["hermes-index"],
            "installed": {}
        }))
        .unwrap();
        assert_eq!(search.results.len(), 1);
        assert_eq!(search.source_counts["github"], json!(3));
        assert_eq!(search.timed_out, vec!["hermes-index"]);

        let preview: SkillPreview = crate::api::decode(json!({
            "name": "x", "description": "", "source": "github", "identifier": "i",
            "trust_level": "community", "repo": "o/r", "tags": ["t"],
            "skill_md": "# x", "files": ["SKILL.md"]
        }))
        .unwrap();
        assert_eq!(preview.skill_md, "# x");
        assert_eq!(preview.files, vec!["SKILL.md"]);

        let scan: SkillScan = crate::api::decode(json!({
            "name": "x", "identifier": "i", "source": "github", "trust_level": "community",
            "verdict": "caution", "summary": "1 finding", "policy": "ask",
            "policy_reason": "medium finding", "findings": [{"severity": "medium",
            "category": "network", "file": "SKILL.md", "line": 4, "description": "curl"}],
            "severity_counts": {"critical": 0, "high": 0, "medium": 1, "low": 0},
            "tier1": null
        }))
        .unwrap();
        assert_eq!(scan.policy, "ask");
        assert_eq!(scan.findings[0].line, Some(4));
        assert_eq!(scan.severity_counts["medium"], 1);

        assert_eq!(
            crate::api::decode::<OfficialSkills>(json!({})).unwrap(),
            OfficialSkills::default()
        );
        assert_eq!(
            crate::api::decode::<HubSearch>(json!({})).unwrap(),
            HubSearch::default()
        );
        assert_eq!(
            crate::api::decode::<SkillScan>(json!({})).unwrap(),
            SkillScan::default()
        );
        assert_eq!(
            crate::api::decode::<SpawnAction>(json!({"ok": true, "pid": 42, "name": "skills-update"}))
                .unwrap()
                .pid,
            Some(42)
        );
    }

    #[test]
    fn skills_rpc_builders() {
        assert_eq!(
            skills_manage_call("list", "", ""),
            Call {
                method: "skills.manage",
                params: json!({"action": "list", "query": ""})
            }
        );
        assert_eq!(
            skills_manage_call("search", "rust", "swe").params,
            json!({"action": "search", "query": "rust", "profile": "swe"})
        );
        assert_eq!(skills_reload_call("", "").params, json!({}));
        assert_eq!(
            skills_reload_call("s1", "swe").params,
            json!({"session_id": "s1", "profile": "swe"})
        );
        let reload: SkillsReload = crate::api::decode(json!({
            "output": "Reloading skills...\n2 skill(s) available",
            "result": {"added": [{"name": "x"}], "removed": [], "total": 2}
        }))
        .unwrap();
        assert_eq!(reload.result.added[0].name, "x");
        assert_eq!(reload.result.total, 2);
        assert_eq!(
            crate::api::decode::<SkillsReload>(json!({})).unwrap(),
            SkillsReload::default()
        );
    }

    #[test]
    fn tools_rest_builders() {
        assert_get(&toolset_config_request("file", ""), "/api/tools/toolsets/file/config");
        assert_get(
            &toolset_config_request("file", "swe"),
            "/api/tools/toolsets/file/config?profile=swe",
        );
        assert_eq!(
            toolset_models_request("image_gen", "nous", "swe").path,
            "/api/tools/toolsets/image_gen/models?provider=nous&profile=swe"
        );
        let m = toolset_select_model_request("image_gen", "flux-2", "nous", "swe");
        assert_eq!(
            (m.method, m.path.as_str()),
            (Method::Put, "/api/tools/toolsets/image_gen/model?profile=swe")
        );
        assert_eq!(
            m.body,
            Some(json!({"model": "flux-2", "provider": "nous", "profile": "swe"}))
        );

        let p = toolset_select_provider_request("web", "firecrawl", "search", "");
        assert_eq!(
            (p.method, p.path.as_str()),
            (Method::Put, "/api/tools/toolsets/web/provider")
        );
        assert_eq!(p.body, Some(json!({"provider": "firecrawl", "capability": "search"})));

        let e = toolset_save_env_request("web", json!({"FIRECRAWL_API_KEY": "k"}), "");
        assert_eq!(
            (e.method, e.path.as_str()),
            (Method::Put, "/api/tools/toolsets/web/env")
        );
        assert_eq!(e.body, Some(json!({"env": {"FIRECRAWL_API_KEY": "k"}})));

        let s = toolset_post_setup_request("image_gen", "install-torch", "swe");
        assert_eq!(
            (s.method, s.path.as_str()),
            (Method::Post, "/api/tools/toolsets/image_gen/post-setup?profile=swe")
        );
        assert_eq!(s.body, Some(json!({"key": "install-torch", "profile": "swe"})));
    }

    #[test]
    fn tools_rest_decodes() {
        let cfg: ToolsetConfig = crate::api::decode(json!({
            "name": "web", "has_category": true, "active_provider": "firecrawl",
            "providers": [{
                "name": "firecrawl", "badge": "recommended", "tag": "managed",
                "env_vars": [{"key": "FIRECRAWL_API_KEY", "prompt": "API key",
                              "url": "https://firecrawl.dev", "default": null, "is_set": true}],
                "post_setup": null, "requires_nous_auth": false, "managed_nous_feature": "web",
                "is_active": true, "status": "ready", "web_backend": "firecrawl",
                "capabilities": {"search": true, "extract": true}
            }],
            "active_search_backend": "firecrawl", "active_extract_backend": "firecrawl",
            "search_via_nous": false, "extract_via_nous": false
        }))
        .unwrap();
        assert_eq!(cfg.providers[0].env_vars[0].key, "FIRECRAWL_API_KEY");
        assert!(cfg.providers[0].is_active);
        assert_eq!(cfg.active_search_backend.as_deref(), Some("firecrawl"));

        let plain: ToolsetConfig =
            crate::api::decode(json!({"name": "file", "has_category": false, "providers": []})).unwrap();
        assert!(!plain.has_category);
        assert_eq!(
            crate::api::decode::<ToolsetConfig>(json!({})).unwrap(),
            ToolsetConfig::default()
        );

        let models: ToolsetModels = crate::api::decode(json!({
            "name": "image_gen", "has_models": true, "provider": "nous", "plugin": "image_gen",
            "models": [{"id": "flux-2", "display": "Flux 2", "speed": "fast",
                        "strengths": "quality", "price": "$0.01"}],
            "current": "flux-2", "default": "flux-2"
        }))
        .unwrap();
        assert_eq!(models.models[0].id, "flux-2");
        assert!(
            !crate::api::decode::<ToolsetModels>(json!({"name": "tts"}))
                .unwrap()
                .has_models
        );
    }

    #[test]
    fn terminal_and_computer_use() {
        assert_get(
            &terminal_backends_request("swe"),
            "/api/tools/terminal/backends?profile=swe",
        );
        let b: TerminalBackends = crate::api::decode(json!({
            "active": "local",
            "backends": [{"name": "local", "label": "Local", "description": "Host shell",
                          "active": true, "status": "ready", "detail": null}]
        }))
        .unwrap();
        assert_eq!(b.active, "local");
        assert!(b.backends[0].active);
        assert_eq!(b.backends[0].detail, None);

        let p = select_terminal_backend_request("docker", "swe");
        assert_eq!(
            (p.method, p.path.as_str(), p.body),
            (
                Method::Put,
                "/api/tools/terminal/backend?profile=swe",
                Some(json!({"backend": "docker", "profile": "swe"}))
            )
        );
        let sel: TerminalBackendSelected = crate::api::decode(json!({"ok": true, "backend": "docker"})).unwrap();
        assert!(sel.ok);

        assert_get(&computer_use_status_request(""), "/api/tools/computer-use/status");
        let cu: ComputerUseStatus = crate::api::decode(json!({
            "platform": "macos", "ready": true, "can_grant": true,
            "checks": [{"name": "screen_recording", "ok": true}]
        }))
        .unwrap();
        assert_eq!(cu.platform, "macos");
        assert_eq!(cu.checks[0]["name"], json!("screen_recording"));
        assert_eq!(
            crate::api::decode::<ComputerUseStatus>(json!({})).unwrap(),
            ComputerUseStatus::default()
        );
        assert_eq!(
            grant_computer_use_permissions_request("swe").path,
            "/api/tools/computer-use/permissions/grant?profile=swe"
        );
    }

    #[test]
    fn tools_rpc_builders() {
        assert_eq!(tools_list_call("").params, json!({}));
        assert_eq!(tools_list_call("s1").params, json!({"session_id": "s1"}));
        assert_eq!(toolsets_list_call("s1").params, json!({"session_id": "s1"}));
        assert_eq!(tools_show_call("").params, json!({}));
        assert_eq!(
            tools_configure_call("enable", &["web", "mcp:fs"], "").params,
            json!({"action": "enable", "names": ["web", "mcp:fs"]})
        );
        let configure: ToolsConfigure = crate::api::decode(json!({
            "changed": ["web"], "enabled_toolsets": ["file", "web"], "info": null,
            "missing_servers": [], "reset": false, "unknown": ["nope"]
        }))
        .unwrap();
        assert_eq!(configure.changed, vec!["web"]);
        assert_eq!(configure.unknown, vec!["nope"]);
    }

    #[test]
    fn tools_rpc_decodes() {
        let list: Toolsets = crate::api::decode(json!({
            "toolsets": [{"name": "file", "description": "File tools", "tool_count": 2,
                          "enabled": true, "tools": ["read_file", "write_file"]}]
        }))
        .unwrap();
        assert_eq!(list.toolsets[0].tool_count, 2);
        assert_eq!(list.toolsets[0].tools.len(), 2);

        let show: ToolsShow = crate::api::decode(json!({
            "sections": [{"name": "file", "tools": [{"name": "read_file", "description": "Read."}]}],
            "total": 1
        }))
        .unwrap();
        assert_eq!(show.sections[0].name, "file");
        assert_eq!(show.total, 1);
        assert_eq!(crate::api::decode::<Toolsets>(json!({})).unwrap(), Toolsets::default());
        assert_eq!(
            crate::api::decode::<ToolsShow>(json!({})).unwrap(),
            ToolsShow::default()
        );
        assert_eq!(
            crate::api::decode::<ToolsConfigure>(json!({})).unwrap(),
            ToolsConfigure::default()
        );
    }

    #[test]
    fn mcp_builders() {
        assert_get(&mcp_catalog_request("swe"), "/api/mcp/catalog?profile=swe");
        let install = mcp_catalog_install_request("fs", json!({"K": "v"}), true, "");
        assert_eq!(
            (install.method, install.path.as_str()),
            (Method::Post, "/api/mcp/catalog/install")
        );
        assert_eq!(
            install.body,
            Some(json!({"name": "fs", "env": {"K": "v"}, "enable": true}))
        );

        let replace = mcp_servers_replace_request(json!({"s": {"command": "echo"}}), "swe");
        assert_eq!(
            (replace.method, replace.path.as_str()),
            (Method::Put, "/api/mcp/servers?profile=swe")
        );
        assert_eq!(
            replace.body,
            Some(json!({"servers": {"s": {"command": "echo"}}, "profile": "swe"}))
        );

        assert_eq!(mcp_servers_list_call("swe").params, json!({"profile": "swe"}));
        assert_eq!(mcp_servers_status_call("").params, json!({}));
        assert_eq!(
            mcp_servers_add_call("n", "fs", Some(json!({"command": "echo"})), "").params,
            json!({"name": "n", "preset": "fs", "config": {"command": "echo"}})
        );
        assert_eq!(
            mcp_servers_remove_call("n", "swe").params,
            json!({"name": "n", "profile": "swe"})
        );
        assert_eq!(
            mcp_servers_set_api_key_call("n", "secret", "", "swe").params,
            json!({"name": "n", "value": "secret", "profile": "swe"})
        );
        assert_eq!(mcp_servers_test_call("n", "").params, json!({"name": "n"}));
        assert_eq!(
            mcp_servers_oauth_start_call("n", "http://127.0.0.1:9/cb", "").params,
            json!({"name": "n", "client_redirect_uri": "http://127.0.0.1:9/cb"})
        );
        assert_eq!(
            mcp_servers_oauth_poll_call("n", "sid", "").params,
            json!({"name": "n", "session_id": "sid"})
        );
        assert_eq!(
            mcp_servers_oauth_cancel_call("n", "sid", "").params,
            json!({"name": "n", "session_id": "sid"})
        );
        assert_eq!(
            mcp_servers_oauth_callback_call("n", "sid", "code", "state", "").params,
            json!({"name": "n", "session_id": "sid", "code": "code", "state": "state"})
        );
        assert_eq!(mcp_catalog_rpc_call("swe").params, json!({"profile": "swe"}));
    }

    #[test]
    fn mcp_decodes() {
        let catalog: McpCatalog = crate::api::decode(json!({
            "entries": [{
                "name": "fs", "description": "Filesystem", "connector_slug": null,
                "source": "official", "transport": "stdio", "auth_type": "none",
                "required_env": [{"name": "FS_ROOT", "prompt": "Root", "required": true}],
                "command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem"],
                "url": null, "install_url": null, "install_ref": null, "bootstrap": [],
                "default_enabled": null, "post_install": "", "suggest": null,
                "needs_install": false, "installed": true, "enabled": true
            }],
            "diagnostics": [{"name": "bad", "kind": "warning", "message": "skipped"}]
        }))
        .unwrap();
        assert_eq!(catalog.entries[0].command.as_deref(), Some("npx"));
        assert!(catalog.entries[0].required_env[0].required);
        assert_eq!(catalog.diagnostics[0].kind, "warning");
        assert_eq!(
            crate::api::decode::<McpCatalog>(json!({})).unwrap(),
            McpCatalog::default()
        );

        let list: McpServersList = crate::api::decode(json!({
            "servers": [{"name": "fs", "transport": "stdio", "url": null, "command": "npx",
                         "args": ["-y", "x"], "env": ["FS_ROOT"], "auth": null,
                         "oauth_tokens_present": null, "enabled": true, "tools": null,
                         "source": "config", "plugin": null}]
        }))
        .unwrap();
        assert_eq!(list.servers[0].env, vec!["FS_ROOT"]);
        assert!(list.servers[0].enabled);

        let status: McpServersStatus = crate::api::decode(json!({
            "servers": [{"name": "fs", "transport": "stdio", "tools": 2, "connected": true,
                         "disabled": false, "status": "connected", "source": "config",
                         "plugin": null}],
            "checked_at": 1_730_000_000_000i64
        }))
        .unwrap();
        assert_eq!(status.checked_at, 1_730_000_000_000);
        assert!(status.servers[0].connected);

        let probe: McpProbe = crate::api::decode(json!({
            "ok": true, "tools": [{"name": "read", "description": "Read"}],
            "prompts": 0, "resources": 0, "oauth_needed": false, "oauth_tokens_present": null
        }))
        .unwrap();
        assert!(probe.ok);
        assert_eq!(probe.tools[0].name, "read");

        let added: McpServerAdded = crate::api::decode(json!({
            "ok": true, "name": "fs", "server": {"name": "fs", "transport": "stdio", "enabled": true}
        }))
        .unwrap();
        assert_eq!(added.server.name, "fs");
        assert_eq!(
            crate::api::decode::<McpServersList>(json!({})).unwrap(),
            McpServersList::default()
        );
        assert_eq!(crate::api::decode::<McpProbe>(json!({})).unwrap(), McpProbe::default());
        assert_eq!(
            crate::api::decode::<McpRpcCatalog>(json!({"servers": [{"name": "fs", "requires": ["K"]}]}))
                .unwrap()
                .servers[0]
                .requires,
            vec!["K"]
        );
    }

    #[test]
    fn learning_builders_and_decodes() {
        assert_get(&learning_graph_request("swe"), "/api/learning/graph?profile=swe");
        assert_eq!(
            learning_node_request("skill:dev/x", "").path,
            "/api/learning/node?id=skill%3Adev%2Fx"
        );
        let u = learning_node_update_request("n1", "new", "swe");
        assert_eq!(
            (u.method, u.path.as_str()),
            (Method::Put, "/api/learning/node?profile=swe")
        );
        assert_eq!(u.body, Some(json!({"id": "n1", "content": "new", "profile": "swe"})));
        let d = learning_node_delete_request("n1", "");
        assert_eq!((d.method, d.path.as_str()), (Method::Delete, "/api/learning/node"));
        assert_eq!(d.body, Some(json!({"id": "n1"})));

        let graph: LearningGraph = crate::api::decode(json!({
            "nodes": [{"id": "n1", "label": "Alpha", "kind": "skill", "timestamp": 1730000000000i64,
                       "category": "dev", "useCount": 3, "state": "active", "createdBy": "agent",
                       "pinned": false, "memorySource": "session"}],
            "edges": [{"source": "n1", "target": "n2"}],
            "clusters": [], "memory": {}, "stats": {"total": 1}
        }))
        .unwrap();
        assert_eq!(graph.nodes[0].use_count, 3);
        assert_eq!(graph.nodes[0].created_by.as_deref(), Some("agent"));
        assert_eq!(graph.edges[0].target, "n2");
        assert_eq!(
            crate::api::decode::<LearningGraph>(json!({})).unwrap(),
            LearningGraph::default()
        );

        let detail: LearningDetail = crate::api::decode(json!({
            "ok": true, "kind": "skill", "id": "n1", "label": "Alpha", "content": "# Alpha"
        }))
        .unwrap();
        assert_eq!(detail.content.as_deref(), Some("# Alpha"));
        assert_eq!(
            crate::api::decode::<LearningDetail>(json!({})).unwrap(),
            LearningDetail::default()
        );

        assert_eq!(
            learning_frames_call(80, 24, 48).params,
            json!({"cols": 80, "rows": 24, "frames": 48})
        );
        assert_eq!(learning_detail_call("n1").params, json!({"id": "n1"}));
        assert_eq!(
            learning_edit_call("n1", "x").params,
            json!({"id": "n1", "content": "x"})
        );
        assert_eq!(learning_delete_call("n1").params, json!({"id": "n1"}));

        let frames: LearningFrames = crate::api::decode(json!({
            "frames": [{"reveal": 1.0, "date": "2026-01-01", "visible": 2, "grid": [], "labels": []}],
            "legend": [{"glyph": "●", "label": "skill", "style": "skill", "color": "#fff"}],
            "categories": [],
            "buckets": [{"index": 0, "label": "Jan", "date": "2026-01", "skills": 1,
                         "memories": 0, "total": 1, "nodes": []}],
            "summary": ["1 skill"], "axis": {"start": "2026-01", "end": "2026-01"},
            "count": 1, "cols": 80, "rows": 24
        }))
        .unwrap();
        assert_eq!(frames.legend[0].label, "skill");
        assert_eq!(frames.axis.unwrap().start, "2026-01");
        assert_eq!(
            crate::api::decode::<LearningFrames>(json!({})).unwrap(),
            LearningFrames::default()
        );
    }

    #[test]
    fn vault_builders_and_decodes() {
        assert_eq!(vault_list_call("").params, json!({}));
        assert_eq!(vault_sources_call("swe").params, json!({"profile": "swe"}));
        assert_eq!(
            vault_add_call("login", "Mail", "https://mail.example", json!({"password": "p"}), "swe").params,
            json!({"kind": "login", "label": "Mail", "origin": "https://mail.example",
                   "secret": {"password": "p"}, "profile": "swe"})
        );
        assert_eq!(vault_remove_call("v1", "").params, json!({"id": "v1"}));
        assert_eq!(
            vault_unlock_call("1password", "pw", "").params,
            json!({"name": "1password", "password": "pw"})
        );
        assert_eq!(vault_lock_call("", "").params, json!({}));
        assert_eq!(
            vault_source_set_call("1password", false, "").params,
            json!({"name": "1password", "enabled": false})
        );

        let list: VaultList = crate::api::decode(json!({
            "items": [{"id": "v1", "kind": "login", "label": "Mail",
                       "origin": "https://mail.example", "created_at": "2026-01-01T00:00:00Z",
                       "backend": "local", "identifier": "me@example.com",
                       "identifier_type": "email", "has_otp": true,
                       "allowed_origins": ["https://mail.example"]}]
        }))
        .unwrap();
        assert_eq!(list.items[0].identifier.as_deref(), Some("me@example.com"));
        assert!(list.items[0].has_otp);
        assert_eq!(
            crate::api::decode::<VaultList>(json!({})).unwrap(),
            VaultList::default()
        );

        let sources: VaultSources = crate::api::decode(json!({
            "sources": [{"name": "local", "display_name": "Hermes vault", "enabled": true,
                         "needs_unlock": false, "unlocked": true, "installed": true}]
        }))
        .unwrap();
        assert!(sources.sources[0].unlocked);
        assert_eq!(
            crate::api::decode::<VaultSources>(json!({})).unwrap(),
            VaultSources::default()
        );
        assert_eq!(
            crate::api::decode::<VaultUnlocked>(json!({"name": "1password", "unlocked": true}))
                .unwrap()
                .name,
            "1password"
        );
    }

    #[test]
    fn browser_manage_builders_and_decode() {
        assert_eq!(
            browser_manage_call("status", None, "", "", "").params,
            json!({ "action": "status" })
        );
        assert_eq!(
            browser_manage_call("use", Some(false), "", "", "swe").params,
            json!({ "action": "use", "enabled": false, "profile": "swe" })
        );
        assert_eq!(
            browser_manage_call("connect", None, "ws://127.0.0.1:9222/devtools/browser/x", "s1", "").params,
            json!({ "action": "connect", "url": "ws://127.0.0.1:9222/devtools/browser/x",
                    "session_id": "s1" })
        );
        assert_eq!(
            browser_manage_call("disconnect", None, "", "", "").params,
            json!({ "action": "disconnect" })
        );

        let b: BrowserManaged = crate::api::decode(json!({
            "connected": true, "url": "http://127.0.0.1:9222", "browser_use": false
        }))
        .unwrap();
        assert!(b.connected);
        assert!(b.messages.is_empty());
        let c: BrowserManaged = crate::api::decode(json!({
            "connected": true, "url": "http://127.0.0.1:9222",
            "messages": ["Adopted 127.0.0.1:9222."], "extra": 1
        }))
        .unwrap();
        assert_eq!(c.messages.len(), 1);
        assert!(!c.browser_use);
        assert_eq!(
            crate::api::decode::<BrowserManaged>(json!({})).unwrap(),
            BrowserManaged::default()
        );
    }
}

#[cfg(test)]
mod rest_gap_tests {
    use super::*;
    use crate::api::decode;

    #[test]
    fn mcp_servers_list_builder() {
        assert_eq!(mcp_servers_request("").path, "/api/mcp/servers");
        let r = mcp_servers_request("swe");
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (Method::Get, "/api/mcp/servers?profile=swe", None)
        );
        let list: McpServerConfigs = decode(json!({
            "servers": [
                {"name": "fs", "transport": "stdio", "url": null, "command": "npx",
                 "args": ["-y", "server-fs"], "env": {"TOKEN": "«redacted»"},
                 "auth": null, "enabled": true, "tools": null, "source": "config",
                 "plugin": null},
                {"name": "remote", "transport": "http", "url": "https://mcp.test/sse",
                 "command": null, "args": [], "env": {}, "auth": "header",
                 "enabled": false, "tools": ["search"], "source": "plugin", "plugin": "web"}
            ]
        }))
        .unwrap();
        assert_eq!(list.servers.len(), 2);
        assert_eq!(list.servers[0].transport, "stdio");
        assert!(list.servers[0].enabled);
        assert_eq!(list.servers[1].tools.as_deref(), Some(&["search".to_string()][..]));
        assert_eq!(list.servers[1].source, "plugin");
        assert_eq!(
            decode::<McpServerConfigs>(json!({})).unwrap(),
            McpServerConfigs::default()
        );
    }
}
