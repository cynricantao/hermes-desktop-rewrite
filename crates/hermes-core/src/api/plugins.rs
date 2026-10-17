//! Plugins and connectors: installed plugins (`plugins.list`, `plugins.manage`), the plugin
//! catalog door, the per-plugin REST namespace (`/api/plugins/<id>/…` and the bundled kanban
//! board), the connectors page (`connectors.*`) and the env/MCP reloads.
//!
//! Follows the conventions in [`crate::api`].
//!
//! # What the backend actually serves (checked against the handlers)
//! * **No `REST /api/plugins` collection.** There is no list / get / enable-disable / delete
//!   REST route: `hermes serve` answers 404 ("web UI disabled") and the dashboard only mounts
//!   one forwarded router per plugin, `/api/plugins/<name>/{path:path}`
//!   (`hermes_cli/web_server_dashboard.py`). Plugin management is RPC-only
//!   (`plugins.manage`), so nothing subclassing a REST CRUD shape is implemented here.
//! * **The per-plugin namespace is real** and is what the desktop's `pluginRest` /
//!   `pluginSocket` doors use ([`plugin_rest_request`]); `GET /api/plugins/kanban/board`
//!   (served by the bundled kanban plugin, `plugins/kanban/dashboard/plugin_api.py`) is
//!   typed here as [`kanban_board`].
//! * `connectors.*` and the reloads are RPC methods registered by `tui_gateway`; result
//!   shapes follow `tui_gateway/contracts/connectors*.py` and `methods_connectors*.py`.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use crate::http::Rest;
use crate::rpc::Client;

use super::{ApiError, Call, Method, Request, call_as, profile_query, req, seg, send, with_query};

/// `profile` param for an RPC call: omitted for the default/launch profile.
fn profile_param(profile: &str) -> Value {
    if profile.is_empty() {
        Value::Null
    } else {
        json!(profile)
    }
}

/// Build a params object, dropping `null` entries so an omitted option stays omitted.
fn params(pairs: &[(&str, Value)]) -> Value {
    let mut map = Map::new();
    for (k, v) in pairs.iter().filter(|(_, v)| !v.is_null()) {
        map.insert((*k).to_owned(), v.clone());
    }
    Value::Object(map)
}

fn opt_str(v: Option<&str>) -> Value {
    v.map(|s| json!(s)).unwrap_or(Value::Null)
}

fn nonempty(s: &str) -> Value {
    if s.is_empty() { Value::Null } else { json!(s) }
}

/// `path` with `?profile=`/`&profile=` appended (empty profile = nothing to add).
fn scoped(path: &str, profile: &str) -> String {
    let q = profile_query(profile);
    if q.is_empty() {
        path.to_owned()
    } else if path.contains('?') {
        format!("{path}&{}", &q[1..])
    } else {
        format!("{path}{q}")
    }
}

fn call_of(method: &'static str, params: Value) -> Call {
    super::call_of(method, params)
}

// ── Installed plugins: plugins.list ─────────────────────────────────────────────

/// One `plugins.list` row: the plugin manager's flat view (legacy; the Plugins Hub uses
/// `plugins.manage list`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct LegacyPluginRow {
    pub name: String,
    pub version: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct PluginsList {
    pub plugins: Vec<LegacyPluginRow>,
}

/// `plugins.list` — loaded plugin manager entries (read-only).
pub fn plugins_list_call() -> Call {
    call_of("plugins.list", json!({}))
}

/// Loaded plugin manager entries (the legacy flat view).
pub async fn plugins_list(client: &Client) -> Result<PluginsList, ApiError> {
    call_as(client, &plugins_list_call()).await
}

// ── Plugins Hub: plugins.manage ─────────────────────────────────────────────────

/// One `plugins.manage list` / toggle / settings row (`methods_tools._plugin_rows`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct PluginRow {
    pub name: String,
    /// Canonical registry key (names collide across category dirs — the UI toggles by key).
    pub key: String,
    pub version: String,
    pub description: String,
    pub source: String,
    pub status: String,
    /// Agent Plugins v1 package.
    pub portable: bool,
    pub install_dir: String,
    /// The package also ships a Desktop UI half (`desktop/plugin.js`).
    pub has_desktop_half: bool,
    pub servers: Vec<PluginServerRow>,
    /// Manifest `config_schema` + current values (secret fields carry no value).
    pub settings_schema: Vec<PluginSettingField>,
    pub catalog_name: Option<String>,
    pub catalog_tier: Option<String>,
    pub installed_sha: Option<String>,
    pub catalog_sha: Option<String>,
    pub catalog_version: Option<String>,
    pub update_available: Option<bool>,
    /// `--ref` installs: the pinned sha so the UI can show the pin.
    pub pinned_sha: Option<String>,
}

/// One MCP server an installed plugin runs, with its honest state sentence.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct PluginServerRow {
    pub name: String,
    /// One of `PluginServerState` (`connected`, `app_not_running`, …).
    pub state: String,
    pub sentence: String,
}

/// One renderable `config_schema` field of a plugin manifest.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct PluginSettingField {
    pub key: String,
    #[serde(rename = "type")]
    pub field_type: String,
    pub label: String,
    pub description: String,
    pub required: bool,
    pub value: Value,
    #[serde(rename = "default")]
    pub default_value: Value,
    pub choices: Option<Vec<String>>,
    /// Secret fields: the `.env` variable the value lives in, and whether it is set.
    pub env: Option<String>,
    pub has_value: Option<bool>,
}

/// What a plugin loaded mid-run does now vs later (`hermes_cli.plugins_activation`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct PluginActivation {
    pub name: String,
    pub key: String,
    /// `{kind: [names]}` live in the running gateway once it reloaded.
    pub activated_now: BTreeMap<String, Vec<String>>,
    /// The plugin's MCP servers and skills, usable from the profile's next turn.
    pub live_now: Option<Value>,
    /// Kinds that apply from the next session (`tools`, `prompt`).
    pub deferred: BTreeMap<String, Vec<String>>,
}

/// A catalog plugin curated for the onboarding card (`plugins.manage onboarding`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct OnboardingPlugin {
    pub name: String,
    pub title: String,
    pub description: String,
    pub tier: String,
    pub platforms: Vec<String>,
    pub app_state: String,
    pub sentence: String,
}

/// A named catalog plugin this host runs (`plugins.manage presence`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct CatalogPluginPresence {
    pub name: String,
    pub title: String,
    pub state: String,
    pub sentence: String,
    pub disclosure: String,
}

/// The one result type every `plugins.manage` action answers with (fields depend on the action).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct PluginsManageResult {
    // list
    pub plugins: Vec<PluginRow>,
    pub user_count: u64,
    pub bundled_count: u64,
    // toggle / update / remove / settings
    pub ok: bool,
    pub unchanged: bool,
    pub restart_required: bool,
    /// The running gateway picked the plugin up and re-wired its handlers.
    pub gateway_reloaded: bool,
    pub activation: Option<PluginActivation>,
    pub cleared_memory_provider: bool,
    /// Canonical key written (toggle/settings) or removed name.
    pub name: String,
    pub plugin: Option<PluginRow>,
    // install / update
    pub plugin_name: String,
    pub warnings: Vec<String>,
    pub missing_env: Vec<String>,
    pub python_dependencies: Vec<String>,
    pub known_issues: Vec<String>,
    pub after_install_path: String,
    pub enabled: Option<bool>,
    pub sha: String,
    /// A re-pin that widened the plugin: nothing changed until the client retries with
    /// `accept_capabilities`.
    pub consent_required: bool,
    pub delta: BTreeMap<String, Vec<String>>,
    pub delta_lines: Vec<String>,
    pub error: String,
    // settings
    pub written: Vec<String>,
    // onboarding / presence
    pub onboarding: Vec<OnboardingPlugin>,
    pub presence: Vec<CatalogPluginPresence>,
}

/// `plugins.manage list` — installed plugins, optionally scoped to a profile's HERMES_HOME.
pub fn plugins_manage_list_call(profile: &str) -> Call {
    call_of(
        "plugins.manage",
        params(&[("action", json!("list")), ("profile", profile_param(profile))]),
    )
}

/// `plugins.manage toggle` — enable/disable one plugin by canonical `key`.
pub fn plugins_manage_toggle_call(key: &str, enable: bool, profile: &str) -> Call {
    call_of(
        "plugins.manage",
        params(&[
            ("action", json!("toggle")),
            ("key", json!(key)),
            ("enable", json!(enable)),
            ("profile", profile_param(profile)),
        ]),
    )
}

/// `plugins.manage install` — git-clone an `identifier`/repo or a curated `catalog_name`.
///
/// **Network** — clones a repository. **Writes to disk.**
pub fn plugins_manage_install_call(
    identifier: &str,
    catalog_name: &str,
    force: bool,
    enable: bool,
    reference: Option<&str>,
    profile: &str,
) -> Call {
    call_of(
        "plugins.manage",
        params(&[
            ("action", json!("install")),
            ("identifier", nonempty(identifier)),
            ("catalog_name", nonempty(catalog_name)),
            ("force", json!(force)),
            ("enable", json!(enable)),
            ("ref", opt_str(reference)),
            ("profile", profile_param(profile)),
        ]),
    )
}

/// `plugins.manage update` — re-pin a catalog install to the current catalog SHA.
///
/// **Network** — fetches the pin and may clone. A widened pin answers `consent_required`
/// first; retry with `accept_capabilities = true`.
pub fn plugins_manage_update_call(name: &str, accept_capabilities: bool, profile: &str) -> Call {
    call_of(
        "plugins.manage",
        params(&[
            ("action", json!("update")),
            ("name", json!(name)),
            (
                "accept_capabilities",
                if accept_capabilities { json!(true) } else { Value::Null },
            ),
            ("profile", profile_param(profile)),
        ]),
    )
}

/// `plugins.manage remove` — uninstall a user install (`<HERMES_HOME>/plugins/<name>`).
///
/// **Destructive** — deletes the plugin directory.
pub fn plugins_manage_remove_call(name: &str, profile: &str) -> Call {
    call_of(
        "plugins.manage",
        params(&[
            ("action", json!("remove")),
            ("name", json!(name)),
            ("profile", profile_param(profile)),
        ]),
    )
}

/// `plugins.manage settings` — write manifest-declared `values` for one plugin.
///
/// **Writes to disk** — non-secret schema keys only; secrets go through the `.env` route.
pub fn plugins_manage_settings_call(key: &str, values: Value, profile: &str) -> Call {
    call_of(
        "plugins.manage",
        params(&[
            ("action", json!("settings")),
            ("key", json!(key)),
            ("values", values),
            ("profile", profile_param(profile)),
        ]),
    )
}

/// `plugins.manage onboarding` — catalog plugins curated for the onboarding card.
///
/// **Network** — reads the live catalog.
pub fn plugins_manage_onboarding_call(profile: &str) -> Call {
    call_of(
        "plugins.manage",
        params(&[("action", json!("onboarding")), ("profile", profile_param(profile))]),
    )
}

/// `plugins.manage presence` — named catalog plugins this host runs, in the order asked.
///
/// **Network** — reads the live catalog.
pub fn plugins_manage_presence_call(names: &[&str], profile: &str) -> Call {
    call_of(
        "plugins.manage",
        params(&[
            ("action", json!("presence")),
            ("names", json!(names)),
            ("profile", profile_param(profile)),
        ]),
    )
}

/// Run one prepared [`plugins.manage`](plugins_manage_list_call) call.
pub async fn plugins_manage(client: &Client, c: &Call) -> Result<PluginsManageResult, ApiError> {
    call_as(client, c).await
}

// ── Plugin catalog: plugins.catalog.lookup ──────────────────────────────────────

/// The fields an install needs for one catalog name (the reviewed pin).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct PluginCatalogEntry {
    pub name: String,
    pub repo: String,
    pub sha: String,
    pub subdir: String,
}

/// `found` carries `entry`; `unknown` covers a name the catalog doesn't list and one it
/// removed; `unavailable` means neither the live catalog nor the in-tree fallback could be read.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct PluginCatalogLookup {
    /// `found` / `invalid_name` / `unknown` / `unavailable`.
    pub status: String,
    pub entry: Option<PluginCatalogEntry>,
}

/// `plugins.catalog.lookup` — resolve one curated catalog name against the backend's live
/// catalog (cache, kill list, in-tree fallback) — the same resolution install uses.
///
/// **Network** — reads the live catalog feed (falls back to the cache / in-tree copy).
pub fn plugins_catalog_lookup_call(name: &str, profile: &str) -> Call {
    call_of(
        "plugins.catalog.lookup",
        params(&[("name", json!(name)), ("profile", profile_param(profile))]),
    )
}

/// One curated catalog entry by name.
pub async fn plugins_catalog_lookup(
    client: &Client,
    name: &str,
    profile: &str,
) -> Result<PluginCatalogLookup, ApiError> {
    call_as(client, &plugins_catalog_lookup_call(name, profile)).await
}

// ── Per-plugin REST namespace: /api/plugins/<id>/… ──────────────────────────────

/// One request into a plugin's own backend namespace, mirroring the desktop's `pluginRest`
/// door: `suffix` is relative to `/api/plugins/<plugin_id>` ('/board' → `/api/plugins/kanban/board`).
///
/// Rejects a `..` segment in the path portion (the namespace is the boundary) — the desktop
/// throws the same way. The reply shape is the plugin's own; use [`send`] / JSON yourself.
pub fn plugin_rest_request(
    plugin_id: &str,
    suffix: &str,
    method: Method,
    body: Option<Value>,
    profile: &str,
) -> Result<Request, String> {
    let suffix = if suffix.starts_with('/') {
        suffix.to_owned()
    } else {
        format!("/{suffix}")
    };
    let path_only = suffix.split(['?', '#']).next().unwrap_or("");
    if path_only.split('/').any(|s| s == "..") {
        return Err(format!("illegal path traversal in {suffix:?}"));
    }
    Ok(req(
        method,
        scoped(&format!("/api/plugins/{}{suffix}", seg(plugin_id)), profile),
        body,
    ))
}

/// Send one plugin-namespace request and return its raw JSON reply.
///
/// The only failure before sending is a rejected `..` segment, reported as [`ApiError::Decode`].
pub async fn plugin_rest(
    rest: &Rest,
    plugin_id: &str,
    suffix: &str,
    method: Method,
    body: Option<Value>,
    profile: &str,
) -> Result<Value, ApiError> {
    let r = plugin_rest_request(plugin_id, suffix, method, body, profile).map_err(ApiError::Decode)?;
    send(rest, &r).await
}

/// One kanban column (`{"name": …, "tasks": [...]}`); tasks are the plugin's own snapshot rows.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct KanbanColumn {
    pub name: String,
    pub tasks: Vec<Value>,
}

/// `GET /api/plugins/kanban/board` — the bundled kanban board grouped by status column.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct KanbanBoard {
    pub columns: Vec<KanbanColumn>,
    pub tenants: Vec<String>,
    /// Known assignees (profiles with tasks or configured on disk), for lane sub-grouping.
    pub assignees: Vec<String>,
    /// Tail cursor a fresh `/events` socket resumes from.
    pub latest_event_id: i64,
    pub now: i64,
}

/// `GET /api/plugins/kanban/board` — `board` empty = the server's current board.
///
/// Read-only; pass `board` on every call so a request never flips the server-wide pointer
/// (same rule as the desktop's kanban plugin).
pub fn kanban_board_request(board: &str, include_archived: bool, profile: &str) -> Request {
    let path = with_query(
        "/api/plugins/kanban/board",
        &[
            ("board", board),
            ("include_archived", if include_archived { "true" } else { "" }),
        ],
    );
    req(Method::Get, scoped(&path, profile), None)
}

/// The bundled kanban plugin's board snapshot (`/api/plugins/kanban/board`).
pub async fn kanban_board(
    rest: &Rest,
    board: &str,
    include_archived: bool,
    profile: &str,
) -> Result<KanbanBoard, ApiError> {
    super::send_as(rest, &kanban_board_request(board, include_archived, profile)).await
}

// ── Connectors: owner + shared rows ─────────────────────────────────────────────

/// Who a `connectors.*` call acts for: the signed-in member (`Account`) or one live session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectorOwner {
    Account,
    Session { session_id: String },
}

impl ConnectorOwner {
    /// A session owner (the session-scoped connector tools).
    pub fn session(session_id: impl Into<String>) -> Self {
        ConnectorOwner::Session {
            session_id: session_id.into(),
        }
    }

    fn to_json(&self) -> Value {
        match self {
            ConnectorOwner::Account => json!({"type": "account"}),
            ConnectorOwner::Session { session_id } => {
                json!({"type": "session", "session_id": session_id})
            }
        }
    }
}

/// One `connectors.list` row: catalog entry plus its connection state.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ConnectorRow {
    pub connector: String,
    pub enabled: bool,
    pub connected: bool,
    /// `pending` / `active` / `failed` / `expired` / `revoked` / `inactive`.
    pub connection_status: Option<String>,
    pub status_reason: Option<String>,
    /// Tools the gateway itself disables for this connector.
    pub gateway_disabled_tools: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ConnectorsListResult {
    pub available: bool,
    pub connectors: Vec<ConnectorRow>,
}

/// One hosted connector account (`connectors.accounts`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ConnectorAccountRow {
    pub connection_id: String,
    pub connector: String,
    pub status: String,
    pub status_reason: Option<String>,
    pub label: String,
    pub alias: Option<String>,
    pub active: bool,
    /// Retired by a reconnect that replaced it; kept, never selectable.
    pub disabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ConnectorAccountsResult {
    pub accounts: Vec<ConnectorAccountRow>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ConnectorAccountsRemoveResult {
    pub connection_id: String,
    pub connector: String,
    pub status: String,
}

/// One catalog entry of the hosted connector directory.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ConnectorCatalogRow {
    pub slug: String,
    pub name: String,
    pub description: String,
    pub category: String,
    pub logo_url: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ConnectorsCatalogResult {
    pub connectors: Vec<ConnectorCatalogRow>,
}

/// One tool of a connector's toolkit.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ConnectorToolRow {
    pub slug: String,
    pub name: String,
    pub description: String,
    /// `read` / `write` / `destructive` / `unclassified`.
    pub facet: String,
    pub hints: Vec<String>,
    pub categories: Vec<String>,
    pub no_auth: bool,
    pub deprecated: bool,
}

/// The scoped profile's cached or current tool list for one connector.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ConnectorToolsResult {
    pub connector: String,
    pub toolkit_version: String,
    pub etag: String,
    pub fetched_at: f64,
    /// `cache` / `network` / `revalidated`.
    pub source: String,
    pub stale: bool,
    pub tools: Vec<ConnectorToolRow>,
}

// ── Connectors: policy ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ConnectorPolicyTags {
    pub enable: Option<Vec<String>>,
    pub disable: Option<Vec<String>>,
}

/// One policy body — one flat lenient struct covering all four `mode` shapes.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ConnectorPolicyBody {
    /// `unrestricted` / `deny-all` / `allow` / `deny`.
    pub mode: String,
    /// `allow` mode: the permitted connectors.
    pub connectors: Vec<String>,
    /// `deny` mode: the denied connectors.
    pub disabled_connectors: Vec<String>,
    /// Per-connector disabled tool names.
    pub tools: BTreeMap<String, Vec<String>>,
    pub tags: Option<ConnectorPolicyTags>,
}

/// The composed policy actually in force for the scoped member.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ConnectorPolicyEffective {
    pub mode: String,
    pub version: u64,
    pub revision: String,
    pub issued_at_ms: i64,
    pub connectors: Vec<String>,
    pub disabled_connectors: Vec<String>,
    pub tools: BTreeMap<String, Vec<String>>,
    pub tags: Option<ConnectorPolicyTags>,
}

/// One policy layer, organization → role → member.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ConnectorPolicyLayer {
    /// `org` / `role` / `member`.
    pub kind: String,
    pub revision: String,
    pub body: ConnectorPolicyBody,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ConnectorPolicyGetResult {
    pub layers: Vec<ConnectorPolicyLayer>,
    pub effective: ConnectorPolicyEffective,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ConnectorPolicySetResult {
    pub revision: String,
    pub effective: ConnectorPolicyEffective,
}

/// One policy change for `connectors.policy.set` (the backend's discriminated
/// `ToolsChange | ConnectorChange`).
#[derive(Debug, Clone, PartialEq)]
pub enum ConnectorPolicyChange {
    /// Replace a connector's disabled-tool list.
    Tools {
        connector: String,
        disabled_tools: Vec<String>,
    },
    /// Enable or disable one connector wholesale.
    Connector { connector: String, enabled: bool },
}

impl ConnectorPolicyChange {
    fn to_json(&self) -> Value {
        match self {
            ConnectorPolicyChange::Tools {
                connector,
                disabled_tools,
            } => json!({"type": "tools", "connector": connector, "disabled_tools": disabled_tools}),
            ConnectorPolicyChange::Connector { connector, enabled } => {
                json!({"type": "connector", "connector": connector, "enabled": enabled})
            }
        }
    }
}

/// `connectors.list` — connector catalog + connection state for one session or account owner.
///
/// **Network** — reads the hosted connector service.
pub fn connectors_list_call(owner: &ConnectorOwner, profile: &str) -> Call {
    call_of(
        "connectors.list",
        params(&[("owner", owner.to_json()), ("profile", profile_param(profile))]),
    )
}

/// Connector catalog + connection state for one owner.
pub async fn connectors_list(
    client: &Client,
    owner: &ConnectorOwner,
    profile: &str,
) -> Result<ConnectorsListResult, ApiError> {
    call_as(client, &connectors_list_call(owner, profile)).await
}

/// `connectors.catalog` — the hosted connector directory available to the scoped member.
///
/// **Network** — reads the hosted connector service.
pub fn connectors_catalog_call(profile: &str) -> Call {
    call_of("connectors.catalog", params(&[("profile", profile_param(profile))]))
}

/// The hosted connector directory.
pub async fn connectors_catalog(client: &Client, profile: &str) -> Result<ConnectorsCatalogResult, ApiError> {
    call_as(client, &connectors_catalog_call(profile)).await
}

/// `connectors.connect` — start or re-initiate authorization for named connectors.
///
/// **Network** — opens an OAuth/authorization operation. `alias` names one account (exactly
/// one connector); `connection_id` repairs an existing unnamed one (needs `reconnect`).
pub fn connectors_connect_call(
    owner: &ConnectorOwner,
    connectors: &[&str],
    reconnect: bool,
    alias: Option<&str>,
    connection_id: Option<&str>,
    profile: &str,
) -> Call {
    call_of(
        "connectors.connect",
        params(&[
            ("owner", owner.to_json()),
            ("connectors", json!(connectors)),
            ("reconnect", json!(reconnect)),
            ("alias", opt_str(alias)),
            ("connection_id", opt_str(connection_id)),
            ("profile", profile_param(profile)),
        ]),
    )
}

/// Start or re-initiate connector authorization; returns the opened operation snapshot.
pub async fn connectors_connect(
    client: &Client,
    owner: &ConnectorOwner,
    connectors: &[&str],
    reconnect: bool,
    alias: Option<&str>,
    connection_id: Option<&str>,
    profile: &str,
) -> Result<ConnectorsConnectResult, ApiError> {
    call_as(
        client,
        &connectors_connect_call(owner, connectors, reconnect, alias, connection_id, profile),
    )
    .await
}

/// `connectors.accounts` — the scoped member's hosted connector accounts.
///
/// **Network** — reads the hosted connector service.
pub fn connectors_accounts_call(connector: Option<&str>, profile: &str) -> Call {
    call_of(
        "connectors.accounts",
        params(&[("connector", opt_str(connector)), ("profile", profile_param(profile))]),
    )
}

/// The scoped member's connector accounts, optionally filtered by connector slug.
pub async fn connectors_accounts(
    client: &Client,
    connector: Option<&str>,
    profile: &str,
) -> Result<ConnectorAccountsResult, ApiError> {
    call_as(client, &connectors_accounts_call(connector, profile)).await
}

/// `connectors.accounts.remove` — remove one hosted connector account.
///
/// **Network**, **Destructive** — revokes and deletes the account.
pub fn connectors_accounts_remove_call(connection_id: &str, profile: &str) -> Call {
    call_of(
        "connectors.accounts.remove",
        params(&[
            ("connection_id", json!(connection_id)),
            ("profile", profile_param(profile)),
        ]),
    )
}

/// Remove one connector account.
pub async fn connectors_accounts_remove(
    client: &Client,
    connection_id: &str,
    profile: &str,
) -> Result<ConnectorAccountsRemoveResult, ApiError> {
    call_as(client, &connectors_accounts_remove_call(connection_id, profile)).await
}

/// `connectors.accounts.rename` — name or rename one hosted connector account.
///
/// **Network** — writes the account alias.
pub fn connectors_accounts_rename_call(connection_id: &str, alias: &str, profile: &str) -> Call {
    call_of(
        "connectors.accounts.rename",
        params(&[
            ("connection_id", json!(connection_id)),
            ("alias", json!(alias)),
            ("profile", profile_param(profile)),
        ]),
    )
}

/// Name or rename one connector account.
pub async fn connectors_accounts_rename(
    client: &Client,
    connection_id: &str,
    alias: &str,
    profile: &str,
) -> Result<ConnectorAccountRow, ApiError> {
    call_as(client, &connectors_accounts_rename_call(connection_id, alias, profile)).await
}

/// `connectors.tools` — one connector's cached or current tool list.
///
/// **Network** — `refresh` re-fetches the toolkit from the hosted service.
pub fn connectors_tools_call(slug: &str, refresh: bool, profile: &str) -> Call {
    call_of(
        "connectors.tools",
        params(&[
            ("slug", json!(slug)),
            ("refresh", json!(refresh)),
            ("profile", profile_param(profile)),
        ]),
    )
}

/// One connector's tool list.
pub async fn connectors_tools(
    client: &Client,
    slug: &str,
    refresh: bool,
    profile: &str,
) -> Result<ConnectorToolsResult, ApiError> {
    call_as(client, &connectors_tools_call(slug, refresh, profile)).await
}

/// `connectors.policy.get` — policy layers for the scoped member.
///
/// **Network** — reads the hosted policy.
pub fn connectors_policy_get_call(profile: &str) -> Call {
    call_of("connectors.policy.get", params(&[("profile", profile_param(profile))]))
}

/// Policy layers, organization → member, plus the effective composition.
pub async fn connectors_policy_get(client: &Client, profile: &str) -> Result<ConnectorPolicyGetResult, ApiError> {
    call_as(client, &connectors_policy_get_call(profile)).await
}

/// `connectors.policy.set` — apply one connector or tool-list change.
///
/// **Network** — writes the member policy; `expected_revision` guards against a concurrent
/// edit (a stale revision answers `policy_conflict`).
pub fn connectors_policy_set_call(change: &ConnectorPolicyChange, expected_revision: &str, profile: &str) -> Call {
    call_of(
        "connectors.policy.set",
        params(&[
            ("change", change.to_json()),
            ("expected_revision", json!(expected_revision)),
            ("profile", profile_param(profile)),
        ]),
    )
}

/// Apply one connector policy change.
pub async fn connectors_policy_set(
    client: &Client,
    change: &ConnectorPolicyChange,
    expected_revision: &str,
    profile: &str,
) -> Result<ConnectorPolicySetResult, ApiError> {
    call_as(client, &connectors_policy_set_call(change, expected_revision, profile)).await
}

// ── Connectors: connection operations ───────────────────────────────────────────

/// One credential an MCP install still needs; the card renders a field per entry.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ConnectorTargetEnvField {
    pub name: String,
    pub required: bool,
    pub secret: bool,
    pub default: String,
    pub prompt: Option<String>,
}

/// One target of an open connection operation (`tools/connectors/contract.py::Target.snapshot`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ConnectionOperationTarget {
    pub name: String,
    /// `connector` / `mcp` / `plugin` / `skill`.
    pub kind: String,
    /// `authorize` / `connect` / `enable` / `install` / `reconnect`.
    pub action: String,
    /// `pending` / `initiated` / `connected` / `skipped` / `failed` / `expired` / `not_connected`.
    pub state: String,
    /// The row needs nothing more from the user (a failed catalog row counts).
    pub resolved: Option<bool>,
    pub detail: Option<String>,
    pub instructions: Option<String>,
    pub discovery_error: Option<String>,
    pub connect_url: Option<String>,
    /// The vendor account a managed mint created or observed.
    pub connection_id: Option<String>,
    pub alias: Option<String>,
    pub attempt: Option<String>,
    pub required_env: Vec<ConnectorTargetEnvField>,
    pub tools: Vec<String>,
    pub hint: Option<String>,
    // Catalog rows (kind plugin / skill) only.
    pub display: Option<String>,
    pub description: Option<String>,
    pub tier: Option<String>,
    pub platforms: Vec<String>,
    pub repo: Option<String>,
    pub sha: Option<String>,
    pub subdir: Option<String>,
    pub requires_hermes: Option<String>,
    pub has_desktop_half: Option<bool>,
    pub target_profile: Option<String>,
    pub app_state: Option<String>,
    pub skill: Option<String>,
    pub phase: Option<String>,
    pub enabled: Option<bool>,
    pub missing_env: Vec<String>,
    pub already_installed: Option<bool>,
    /// The catalog's security scan / the user's Advanced choices (free-form sidecars).
    pub scan: Value,
    pub approved: Value,
    /// MCP servers an installed plugin brought that did not connect.
    pub server_errors: Value,
}

/// An open connection operation's full snapshot (`methods_connectors._operation_view`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ConnectionOperationStatus {
    pub op_id: String,
    /// Monotonic write counter; a frame whose seq is not higher is older.
    pub seq: i64,
    pub deadline_at: f64,
    pub settled: bool,
    pub settled_at: Option<f64>,
    /// `all_resolved` / `continue` / `deadline` / `interrupt`.
    pub settled_by: Option<String>,
    pub targets: Vec<ConnectionOperationTarget>,
}

/// `connectors.connect`'s reply: the operation it opened, plus the tool result's own
/// `status`/`note`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ConnectorsConnectResult {
    #[serde(flatten)]
    pub operation: ConnectionOperationStatus,
    /// `initiated` / `settled`.
    pub status: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ConnectorWakeResult {
    pub status: String,
}

/// `connectors.operation.status` — the current snapshot of one open operation.
///
/// **Network** — reads the hosted connector service.
pub fn connectors_operation_status_call(owner: &ConnectorOwner, op_id: &str, profile: &str) -> Call {
    call_of(
        "connectors.operation.status",
        params(&[
            ("owner", owner.to_json()),
            ("op_id", json!(op_id)),
            ("profile", profile_param(profile)),
        ]),
    )
}

/// The snapshot of one open connection operation.
pub async fn connectors_operation_status(
    client: &Client,
    owner: &ConnectorOwner,
    op_id: &str,
    profile: &str,
) -> Result<ConnectionOperationStatus, ApiError> {
    call_as(client, &connectors_operation_status_call(owner, op_id, profile)).await
}

/// `connectors.operation.wake` — the browser leg came back (`hermes://connections/done`);
/// read the accounts now, not at the next tick.
///
/// **Network** — wakes the operation's watcher.
pub fn connectors_operation_wake_call(owner: &ConnectorOwner, op_id: &str, profile: &str) -> Call {
    call_of(
        "connectors.operation.wake",
        params(&[
            ("owner", owner.to_json()),
            ("op_id", json!(op_id)),
            ("profile", profile_param(profile)),
        ]),
    )
}

/// Wake one open connection operation.
pub async fn connectors_operation_wake(
    client: &Client,
    owner: &ConnectorOwner,
    op_id: &str,
    profile: &str,
) -> Result<ConnectorWakeResult, ApiError> {
    call_as(client, &connectors_operation_wake_call(owner, op_id, profile)).await
}

// ── Reloads ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ReloadEnvResult {
    /// Number of `.env` entries applied from disk.
    pub updated: u64,
}

/// `reload.env` — re-read the backend's `.env` (CLI `/reload` parity); built agents keep
/// their pool, `/new` resolves fresh.
pub fn reload_env_call(session_id: Option<&str>, profile: &str) -> Call {
    call_of(
        "reload.env",
        params(&[("session_id", opt_str(session_id)), ("profile", profile_param(profile))]),
    )
}

/// Re-read the backend's `.env` from disk.
pub async fn reload_env(client: &Client, session_id: Option<&str>, profile: &str) -> Result<ReloadEnvResult, ApiError> {
    call_as(client, &reload_env_call(session_id, profile)).await
}

/// `reload.mcp`'s reply: without `confirm` the backend may answer `confirm_required`
/// (per `approvals.mcp_reload_confirm`) before doing anything.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ReloadMcpResult {
    /// `confirm_required` / `reloaded`.
    pub status: String,
    pub message: Option<String>,
    pub loaded_rev: Option<String>,
    pub coalesced: Option<bool>,
    pub turn_isolation: Option<bool>,
    pub host_ack: Value,
}

/// `reload.mcp` — tear down and rediscover MCP servers for every live session.
///
/// **Spawns processes** (stdio MCP servers reconnect) and invalidates every open chat's
/// prompt cache. Without `confirm` the backend answers `confirm_required` first; `always`
/// persists the opt-out.
pub fn reload_mcp_call(
    session_id: Option<&str>,
    confirm: bool,
    always: bool,
    rev: Option<&str>,
    profile: &str,
) -> Call {
    call_of(
        "reload.mcp",
        params(&[
            ("session_id", opt_str(session_id)),
            ("confirm", if confirm { json!(true) } else { Value::Null }),
            ("always", if always { json!(true) } else { Value::Null }),
            ("rev", opt_str(rev)),
            ("profile", profile_param(profile)),
        ]),
    )
}

/// Tear down and rediscover MCP servers for every live session.
pub async fn reload_mcp(
    client: &Client,
    session_id: Option<&str>,
    confirm: bool,
    always: bool,
    rev: Option<&str>,
    profile: &str,
) -> Result<ReloadMcpResult, ApiError> {
    call_as(client, &reload_mcp_call(session_id, confirm, always, rev, profile)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::decode;

    // ── plugins.list ────────────────────────────────────────────────────────────

    #[test]
    fn plugins_list_shape_and_decode() {
        let c = plugins_list_call();
        assert_eq!(c.method, "plugins.list");
        assert_eq!(c.params, json!({}));

        let l: PluginsList = decode(json!({"plugins": [
            {"name": "kanban", "version": "1.2.3", "enabled": true},
            {"name": "disk-cleanup", "version": "?", "enabled": false, "extra": 1}
        ]}))
        .unwrap();
        assert_eq!(l.plugins.len(), 2);
        assert_eq!(l.plugins[0].name, "kanban");
        assert!(l.plugins[0].enabled);
        assert_eq!(l.plugins[1].version, "?");
        assert!(!l.plugins[1].enabled);
        assert_eq!(decode::<PluginsList>(json!({})).unwrap(), PluginsList::default());
        assert!(decode::<PluginsList>(json!([1])).is_err());
    }

    // ── plugins.manage builders ─────────────────────────────────────────────────

    #[test]
    fn plugins_manage_list_and_toggle_params() {
        assert_eq!(plugins_manage_list_call("").params, json!({"action": "list"}));
        assert_eq!(
            plugins_manage_list_call("swe").params,
            json!({"action": "list", "profile": "swe"})
        );
        assert_eq!(
            plugins_manage_toggle_call("kanban", false, "").params,
            json!({"action": "toggle", "key": "kanban", "enable": false})
        );
        assert_eq!(
            plugins_manage_toggle_call("kanban", true, "ea").params,
            json!({"action": "toggle", "key": "kanban", "enable": true, "profile": "ea"})
        );
    }

    #[test]
    fn plugins_manage_install_update_remove_params() {
        assert_eq!(
            plugins_manage_install_call("owner/repo", "", true, false, Some("v1.2"), "swe").params,
            json!({"action": "install", "identifier": "owner/repo", "force": true, "enable": false,
                   "ref": "v1.2", "profile": "swe"})
        );
        assert_eq!(
            plugins_manage_install_call("", "kanban", false, true, None, "").params,
            json!({"action": "install", "catalog_name": "kanban", "force": false, "enable": true})
        );
        assert_eq!(
            plugins_manage_update_call("kanban", false, "").params,
            json!({"action": "update", "name": "kanban"})
        );
        assert_eq!(
            plugins_manage_update_call("kanban", true, "").params,
            json!({"action": "update", "name": "kanban", "accept_capabilities": true})
        );
        assert_eq!(
            plugins_manage_remove_call("kanban", "").params,
            json!({"action": "remove", "name": "kanban"})
        );
    }

    #[test]
    fn plugins_manage_settings_onboarding_presence_params() {
        assert_eq!(
            plugins_manage_settings_call("kanban", json!({"theme": "dark"}), "ea").params,
            json!({"action": "settings", "key": "kanban", "values": {"theme": "dark"}, "profile": "ea"})
        );
        assert_eq!(
            plugins_manage_onboarding_call("").params,
            json!({"action": "onboarding"})
        );
        assert_eq!(
            plugins_manage_presence_call(&["kanban", "cloudflare"], "").params,
            json!({"action": "presence", "names": ["kanban", "cloudflare"]})
        );
    }

    // ── plugins.manage decode ───────────────────────────────────────────────────

    #[test]
    fn plugins_manage_list_decode() {
        let r: PluginsManageResult = decode(json!({
            "plugins": [{
                "name": "kanban", "key": "kanban/kanban", "version": "1.2.3",
                "description": "Kanban board", "source": "bundled", "status": "enabled",
                "portable": false, "install_dir": "/home/x/plugins/kanban",
                "has_desktop_half": true,
                "servers": [{"name": "kanban", "state": "connected", "sentence": "Serving."}],
                "settings_schema": [{
                    "key": "theme", "type": "string", "label": "Theme", "description": "",
                    "required": false, "value": "dark", "default": "light"
                }, {
                    "key": "token", "type": "secret", "label": "API token", "description": "",
                    "required": true, "env": "KANBAN_TOKEN", "has_value": true
                }],
                "catalog_name": "kanban", "catalog_tier": "official", "installed_sha": "abc",
                "catalog_sha": "abc", "catalog_version": "1.2.3", "update_available": false,
                "pinned_sha": "def", "unknown_extra": true
            }],
            "user_count": 0, "bundled_count": 1
        }))
        .unwrap();
        assert_eq!(r.plugins.len(), 1);
        assert_eq!(r.plugins[0].key, "kanban/kanban");
        assert!(r.plugins[0].has_desktop_half);
        assert_eq!(r.plugins[0].servers[0].state, "connected");
        assert_eq!(r.plugins[0].settings_schema[1].field_type, "secret");
        assert_eq!(r.plugins[0].settings_schema[1].env.as_deref(), Some("KANBAN_TOKEN"));
        assert_eq!(r.plugins[0].settings_schema[0].value, json!("dark"));
        assert_eq!(r.plugins[0].catalog_tier.as_deref(), Some("official"));
        assert_eq!(r.bundled_count, 1);
    }

    #[test]
    fn plugins_manage_toggle_install_consent_decode() {
        let t: PluginsManageResult = decode(json!({
            "ok": true, "unchanged": false, "restart_required": false, "gateway_reloaded": true,
            "activation": {"name": "kanban", "key": "kanban/kanban",
                           "activated_now": {"gateway_commands": ["kanban"]},
                           "deferred": {"tools": ["kanban_write"]}},
            "name": "kanban/kanban",
            "plugin": {"name": "kanban", "key": "kanban/kanban"}
        }))
        .unwrap();
        assert!(t.ok && t.gateway_reloaded);
        let a = t.activation.unwrap();
        assert_eq!(a.activated_now["gateway_commands"], vec!["kanban"]);
        assert_eq!(a.deferred["tools"], vec!["kanban_write"]);
        assert_eq!(t.plugin.unwrap().name, "kanban");

        let consent: PluginsManageResult = decode(json!({
            "ok": false, "consent_required": true, "name": "demo", "sha": "abc",
            "delta": {"hooks": ["on_turn"]}, "delta_lines": ["+ hook on_turn"]
        }))
        .unwrap();
        assert!(consent.consent_required);
        assert_eq!(consent.delta["hooks"], vec!["on_turn"]);
        assert_eq!(consent.delta_lines.len(), 1);

        let install: PluginsManageResult = decode(json!({
            "ok": true, "plugin_name": "demo", "enabled": true, "missing_env": ["DEMO_KEY"],
            "python_dependencies": ["requests"], "known_issues": [], "warnings": ["no lockfile"],
            "after_install_path": "/home/x/plugins/demo"
        }))
        .unwrap();
        assert_eq!(install.plugin_name, "demo");
        assert_eq!(install.missing_env, vec!["DEMO_KEY"]);
        assert_eq!(install.enabled, Some(true));

        let settings: PluginsManageResult = decode(json!({
            "ok": true, "name": "kanban/kanban", "written": ["theme"]
        }))
        .unwrap();
        assert_eq!(settings.written, vec!["theme"]);

        let empty: PluginsManageResult = decode(json!({})).unwrap();
        assert_eq!(empty, PluginsManageResult::default());
        assert!(decode::<PluginsManageResult>(json!("nope")).is_err());
    }

    #[test]
    fn plugins_manage_onboarding_and_presence_decode() {
        let r: PluginsManageResult = decode(json!({
            "onboarding": [{"name": "chrome", "title": "Chrome control", "description": "d",
                            "tier": "official", "platforms": ["linux"], "app_state": "present",
                            "sentence": ""}]
        }))
        .unwrap();
        assert_eq!(r.onboarding[0].name, "chrome");
        assert_eq!(r.onboarding[0].app_state, "present");

        let p: PluginsManageResult = decode(json!({
            "presence": [{"name": "chrome", "title": "Chrome control", "state": "missing_app",
                          "sentence": "Chrome is not installed.", "disclosure": "D."}]
        }))
        .unwrap();
        assert_eq!(p.presence[0].state, "missing_app");
        assert_eq!(p.presence[0].disclosure, "D.");
    }

    // ── plugins.catalog.lookup ──────────────────────────────────────────────────

    #[test]
    fn catalog_lookup_shape_and_decode() {
        assert_eq!(
            plugins_catalog_lookup_call("kanban", "").params,
            json!({"name": "kanban"})
        );
        assert_eq!(
            plugins_catalog_lookup_call("kanban", "ea").params,
            json!({"name": "kanban", "profile": "ea"})
        );
        let found: PluginCatalogLookup = decode(json!({
            "status": "found",
            "entry": {"name": "kanban", "repo": "NousResearch/hermes-plugins",
                      "sha": "0123456789abcdef", "subdir": "kanban", "extra": true}
        }))
        .unwrap();
        assert_eq!(found.status, "found");
        assert_eq!(found.entry.unwrap().subdir, "kanban");
        let unknown: PluginCatalogLookup = decode(json!({"status": "unknown"})).unwrap();
        assert!(unknown.entry.is_none());
        assert_eq!(
            decode::<PluginCatalogLookup>(json!({})).unwrap(),
            PluginCatalogLookup::default()
        );
    }

    // ── per-plugin REST namespace + kanban board ────────────────────────────────

    #[test]
    fn plugin_rest_request_builds_namespace_path() {
        let r = plugin_rest_request("kanban", "/board", Method::Get, None, "").unwrap();
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (Method::Get, "/api/plugins/kanban/board", None)
        );

        let r = plugin_rest_request(
            "kanban",
            "tasks/abc",
            Method::Patch,
            Some(json!({"status": "done"})),
            "ea",
        )
        .unwrap();
        assert_eq!(r.path, "/api/plugins/kanban/tasks/abc?profile=ea");
        assert_eq!(r.body, Some(json!({"status": "done"})));

        let r = plugin_rest_request("hermes achievements", "/x", Method::Get, None, "").unwrap();
        assert_eq!(r.path, "/api/plugins/hermes%20achievements/x");

        // Traversal is rejected on the path portion, allowed in a query value. The check
        // matches the desktop's: an exact `..` segment only (an encoded `..%2F` is one
        // segment and the router decides what it means).
        assert!(plugin_rest_request("kanban", "/../config", Method::Get, None, "").is_err());
        assert!(plugin_rest_request("kanban", "/a/../../config", Method::Get, None, "").is_err());
        assert!(plugin_rest_request("kanban", "/board?board=../x", Method::Get, None, "").is_ok());
        assert!(plugin_rest_request("kanban", "/a/..%2Fb", Method::Get, None, "").is_ok());
    }

    #[test]
    fn kanban_board_shape_and_decode() {
        let r = kanban_board_request("", false, "");
        assert_eq!((r.method, r.path.as_str()), (Method::Get, "/api/plugins/kanban/board"));
        assert_eq!(
            kanban_board_request("work items", true, "swe").path,
            "/api/plugins/kanban/board?board=work%20items&include_archived=true&profile=swe"
        );

        let b: KanbanBoard = decode(json!({
            "assignees": ["default", "swe"],
            "columns": [
                {"name": "triage", "tasks": []},
                {"name": "running", "tasks": [{"id": "t1", "title": "x", "status": "running"}]}
            ],
            "latest_event_id": 42, "now": 1792237312, "tenants": ["acme"]
        }))
        .unwrap();
        assert_eq!(b.columns.len(), 2);
        assert_eq!(b.columns[1].tasks[0]["id"], "t1");
        assert_eq!(b.latest_event_id, 42);
        assert_eq!(b.tenants, vec!["acme"]);
        assert_eq!(decode::<KanbanBoard>(json!({})).unwrap(), KanbanBoard::default());
        assert!(decode::<KanbanBoard>(json!(0)).is_err());
    }

    // ── connectors: owners + reads ──────────────────────────────────────────────

    #[test]
    fn connectors_list_and_catalog_params() {
        assert_eq!(
            connectors_list_call(&ConnectorOwner::Account, "").params,
            json!({"owner": {"type": "account"}})
        );
        assert_eq!(
            connectors_list_call(&ConnectorOwner::session("sess-1"), "swe").params,
            json!({"owner": {"type": "session", "session_id": "sess-1"}, "profile": "swe"})
        );
        assert_eq!(connectors_catalog_call("").params, json!({}));
        assert_eq!(connectors_catalog_call("ea").params, json!({"profile": "ea"}));
        assert_eq!(connectors_accounts_call(None, "").params, json!({}));
        assert_eq!(
            connectors_accounts_call(Some("notion"), "").params,
            json!({"connector": "notion"})
        );
        assert_eq!(
            connectors_tools_call("notion", true, "swe").params,
            json!({"slug": "notion", "refresh": true, "profile": "swe"})
        );
    }

    #[test]
    fn connectors_reads_decode() {
        let l: ConnectorsListResult = decode(json!({
            "available": true,
            "connectors": [{"connector": "notion", "enabled": true, "connected": false,
                            "connection_status": null, "status_reason": null,
                            "gateway_disabled_tools": []}]
        }))
        .unwrap();
        assert!(l.available);
        assert_eq!(l.connectors[0].connector, "notion");
        assert!(l.connectors[0].connection_status.is_none());

        let c: ConnectorsCatalogResult = decode(json!({
            "connectors": [{"slug": "airtable", "name": "Airtable", "description": "bases",
                            "category": "Files and docs", "logo_url": "https://x/y"}]
        }))
        .unwrap();
        assert_eq!(c.connectors[0].slug, "airtable");

        let a: ConnectorAccountsResult = decode(json!({
            "accounts": [{"connection_id": "ca_1", "connector": "notion", "status": "active",
                          "status_reason": null, "label": "work@example.com", "alias": "work",
                          "active": true, "disabled": false,
                          "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-02T00:00:00Z"}]
        }))
        .unwrap();
        assert_eq!(a.accounts[0].alias.as_deref(), Some("work"));

        let t: ConnectorToolsResult = decode(json!({
            "connector": "notion", "toolkit_version": "20261008_00", "etag": "W/\"abc\"",
            "fetched_at": 1792237776.4, "source": "network", "stale": false,
            "tools": [{"slug": "notion_create_page", "name": "Create page", "description": "d",
                       "facet": "write", "hints": [], "categories": [], "no_auth": false,
                       "deprecated": false}]
        }))
        .unwrap();
        assert_eq!(t.source, "network");
        assert_eq!(t.tools[0].facet, "write");

        let r: ConnectorAccountsRemoveResult = decode(json!({
            "connection_id": "ca_1", "connector": "notion", "status": "removed"
        }))
        .unwrap();
        assert_eq!(r.status, "removed");

        assert_eq!(
            decode::<ConnectorsListResult>(json!({})).unwrap(),
            ConnectorsListResult::default()
        );
        assert_eq!(
            decode::<ConnectorToolsResult>(json!({})).unwrap(),
            ConnectorToolsResult::default()
        );
        assert_eq!(
            decode::<ConnectorAccountsResult>(json!({})).unwrap(),
            ConnectorAccountsResult::default()
        );
    }

    // ── connectors: policy ──────────────────────────────────────────────────────

    #[test]
    fn connectors_policy_params_and_decode() {
        assert_eq!(connectors_policy_get_call("").params, json!({}));
        assert_eq!(connectors_policy_get_call("swe").params, json!({"profile": "swe"}));

        let tools = ConnectorPolicyChange::Tools {
            connector: "notion".into(),
            disabled_tools: vec!["notion_delete_page".into()],
        };
        assert_eq!(
            connectors_policy_set_call(&tools, "00000000000000000000000000", "").params,
            json!({"change": {"type": "tools", "connector": "notion",
                              "disabled_tools": ["notion_delete_page"]},
                   "expected_revision": "00000000000000000000000000"})
        );
        let conn = ConnectorPolicyChange::Connector {
            connector: "slack".into(),
            enabled: false,
        };
        assert_eq!(
            connectors_policy_set_call(&conn, "00000000000000000000000000", "ea").params,
            json!({"change": {"type": "connector", "connector": "slack", "enabled": false},
                   "expected_revision": "00000000000000000000000000", "profile": "ea"})
        );

        let got: ConnectorPolicyGetResult = decode(json!({
            "effective": {"issued_at_ms": 1792237311192i64, "mode": "unrestricted",
                          "revision": "00000000000000000000000000", "version": 1},
            "layers": [
                {"body": {"mode": "unrestricted"}, "kind": "org",
                 "revision": "00000000000000000000000000"},
                {"body": {"mode": "deny", "disabled_connectors": ["slack"],
                          "tools": {"notion": ["notion_delete_page"]},
                          "tags": {"enable": ["read"], "disable": null}},
                 "kind": "member", "revision": "00000000000000000000000001"}
            ]
        }))
        .unwrap();
        assert_eq!(got.layers.len(), 2);
        assert_eq!(got.layers[1].body.disabled_connectors, vec!["slack"]);
        assert_eq!(got.layers[1].body.tools["notion"], vec!["notion_delete_page"]);
        assert_eq!(got.effective.mode, "unrestricted");
        assert_eq!(got.effective.version, 1);

        let set: ConnectorPolicySetResult = decode(json!({
            "revision": "00000000000000000000000002",
            "effective": {"mode": "allow", "version": 1, "revision": "00000000000000000000000002",
                          "issued_at_ms": 1, "connectors": ["notion"], "tools": {}}
        }))
        .unwrap();
        assert_eq!(set.effective.mode, "allow");
        assert_eq!(set.effective.connectors, vec!["notion"]);

        assert_eq!(
            decode::<ConnectorPolicyGetResult>(json!({})).unwrap(),
            ConnectorPolicyGetResult::default()
        );
        assert_eq!(
            decode::<ConnectorPolicySetResult>(json!({})).unwrap(),
            ConnectorPolicySetResult::default()
        );
    }

    // ── connectors: operations + connect ────────────────────────────────────────

    #[test]
    fn connectors_operation_params_and_decode() {
        let owner = ConnectorOwner::Account;
        assert_eq!(
            connectors_operation_status_call(&owner, "op-1", "").params,
            json!({"owner": {"type": "account"}, "op_id": "op-1"})
        );
        assert_eq!(
            connectors_operation_wake_call(&owner, "op-1", "swe").params,
            json!({"owner": {"type": "account"}, "op_id": "op-1", "profile": "swe"})
        );

        assert_eq!(
            connectors_connect_call(&owner, &["notion"], false, None, None, "").params,
            json!({"owner": {"type": "account"}, "connectors": ["notion"], "reconnect": false})
        );
        assert_eq!(
            connectors_connect_call(&owner, &["notion"], false, Some("work"), None, "").params,
            json!({"owner": {"type": "account"}, "connectors": ["notion"], "reconnect": false,
                   "alias": "work"})
        );
        assert_eq!(
            connectors_connect_call(&owner, &["slack"], true, None, Some("ca_9"), "ea").params,
            json!({"owner": {"type": "account"}, "connectors": ["slack"], "reconnect": true,
                   "connection_id": "ca_9", "profile": "ea"})
        );

        let s: ConnectionOperationStatus = decode(json!({
            "op_id": "op-1", "seq": 3, "deadline_at": 1792240000.5, "settled": false,
            "settled_at": null, "settled_by": null,
            "targets": [{"name": "notion", "kind": "connector", "action": "connect",
                         "state": "initiated", "resolved": false,
                         "connect_url": "https://portal/x", "alias": "work",
                         "required_env": [], "tools": ["notion_create_page"],
                         "unknown_field": "ignored"}]
        }))
        .unwrap();
        assert_eq!(s.targets[0].state, "initiated");
        assert_eq!(s.targets[0].tools, vec!["notion_create_page"]);

        let c: ConnectorsConnectResult = decode(json!({
            "op_id": "op-1", "seq": 4, "deadline_at": 1792240000.5, "settled": false,
            "targets": [], "status": "initiated", "note": "Open the link."
        }))
        .unwrap();
        assert_eq!(c.operation.op_id, "op-1");
        assert_eq!(c.status.as_deref(), Some("initiated"));
        assert_eq!(c.note.as_deref(), Some("Open the link."));

        let w: ConnectorWakeResult = decode(json!({"status": "ok"})).unwrap();
        assert_eq!(w.status, "ok");

        assert_eq!(
            decode::<ConnectionOperationStatus>(json!({})).unwrap(),
            ConnectionOperationStatus::default()
        );
        assert_eq!(
            decode::<ConnectorsConnectResult>(json!({})).unwrap(),
            ConnectorsConnectResult::default()
        );
        assert_eq!(
            decode::<ConnectionOperationTarget>(json!({})).unwrap(),
            ConnectionOperationTarget::default()
        );
    }

    // ── reloads ─────────────────────────────────────────────────────────────────

    #[test]
    fn reload_params_and_decode() {
        assert_eq!(reload_env_call(None, "").params, json!({}));
        assert_eq!(
            reload_env_call(Some("sess-1"), "swe").params,
            json!({"session_id": "sess-1", "profile": "swe"})
        );
        let e: ReloadEnvResult = decode(json!({"updated": 1})).unwrap();
        assert_eq!(e.updated, 1);

        assert_eq!(reload_mcp_call(None, false, false, None, "").params, json!({}));
        assert_eq!(
            reload_mcp_call(Some("sess-1"), true, false, Some("rev-7"), "").params,
            json!({"session_id": "sess-1", "confirm": true, "rev": "rev-7"})
        );
        assert_eq!(
            reload_mcp_call(None, false, true, None, "").params,
            json!({"always": true})
        );
        let gate: ReloadMcpResult = decode(json!({
            "status": "confirm_required", "message": "⚠️ /reload-mcp invalidates the prompt cache"
        }))
        .unwrap();
        assert_eq!(gate.status, "confirm_required");
        let done: ReloadMcpResult = decode(json!({
            "status": "reloaded", "turn_isolation": true, "host_ack": {"ok": true},
            "loaded_rev": "rev-7", "coalesced": false
        }))
        .unwrap();
        assert_eq!(done.loaded_rev.as_deref(), Some("rev-7"));
        assert_eq!(done.turn_isolation, Some(true));
        assert_eq!(done.host_ack, json!({"ok": true}));
        assert_eq!(
            decode::<ReloadMcpResult>(json!({})).unwrap(),
            ReloadMcpResult::default()
        );
        assert_eq!(
            decode::<ReloadEnvResult>(json!({})).unwrap(),
            ReloadEnvResult::default()
        );
    }
}
