//! Sessions and turns beyond chat basics: session RPCs (history, branch, archive, steer, undo,
//! compress, usage, status, close, cwd, hidden, events, foreign import), REST session
//! listing/search/messages paging, approvals/clarify/request answers, completions, command
//! catalog, prompt background/btw, message reactions.
//!
//! Follows the conventions in [`crate::api`]. Which endpoints were exercised against the
//! private test backend is recorded in `docs/api/sessions.md`.
//!
//! Not covered here (owned by the chat-basics area): `session.create`, `session.resume`,
//! `session.list`, `session.delete`, `session.interrupt`, `session.title`, `prompt.submit`,
//! `approval.respond`, `slash.exec`, `command.dispatch` and every attachment method.

use std::collections::HashMap;

use serde_json::{Value, json};

use super::{ApiError, Call, Method, Request, call_as, call_of, profile_query, req, seg, send_as, with_query};
use crate::http::Rest;
use crate::rpc::Client;

/// Add `profile` to an RPC params object unless it is the default (`""`) one.
fn profiled(mut params: Value, profile: &str) -> Value {
    if !profile.is_empty()
        && let Value::Object(map) = &mut params
    {
        map.insert("profile".into(), Value::String(profile.to_owned()));
    }
    params
}

/// Accept `true`/`false` and the store's `0`/`1` integers for a flag field.
fn bool_from_any<'de, D>(d: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = <Value as serde::Deserialize>::deserialize(d)?;
    Ok(match v {
        Value::Bool(b) => b,
        Value::Number(n) => n.as_i64().unwrap_or(0) != 0,
        _ => false,
    })
}

/// Same as [`bool_from_any`] for an optional flag (`null` stays `None`).
fn opt_bool_from_any<'de, D>(d: D) -> Result<Option<bool>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = <Value as serde::Deserialize>::deserialize(d)?;
    Ok(match v {
        Value::Bool(b) => Some(b),
        Value::Number(n) => Some(n.as_i64().unwrap_or(0) != 0),
        _ => None,
    })
}

// ── Shared row types ────────────────────────────────────────────────────────────

/// One transcript row as the gateway projects it (`role`, display `text`, durable `row_id`).
/// `content` is free-form (string or multimodal parts) and `text` is the display projection.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct Message {
    pub role: String,
    pub content: Value,
    pub text: Value,
    pub display_content: Value,
    pub display_commentary: Value,
    pub display_reasoning: Option<String>,
    pub display_kind: Option<String>,
    pub display_metadata: Value,
    pub row_id: Option<i64>,
    pub id: Option<i64>,
    pub timestamp: Option<f64>,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub name: Option<String>,
    pub tool_calls: Value,
    pub reasoning: Option<String>,
    pub args: Value,
    pub labels: Value,
}

/// Live runtime info (`SessionLiveInfo`): model/workspace/config of a live session.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct LiveInfo {
    pub model: String,
    pub provider: String,
    pub reasoning_effort: String,
    pub reasoning_effort_wire: String,
    pub service_tier: String,
    pub fast: bool,
    pub yolo: bool,
    pub approval_mode: String,
    pub tools: Value,
    pub skills: Value,
    pub cwd: String,
    pub branch: Option<String>,
    pub project: Option<ProjectRef>,
    pub terminal_backend: String,
    pub personality: String,
    pub running: bool,
    pub turn_started_at: Option<f64>,
    pub title: String,
    pub stored_session_id: String,
    pub desktop_contract: Value,
    pub version: String,
    pub release_date: String,
    pub update_behind: Value,
    pub update_command: String,
    pub usage: Option<Usage>,
    pub profile_name: Option<String>,
    pub mcp_servers: Vec<Value>,
    pub system_prompt: Option<String>,
    pub credential_warning: Option<String>,
    pub lazy: Option<bool>,
}

/// A declared project a session's cwd belongs to.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ProjectRef {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub primary_path: Option<String>,
}

/// Token/cost counters for one session (`session.usage`, `SessionLiveInfo.usage`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct Usage {
    pub model: String,
    pub input: i64,
    pub output: i64,
    pub reasoning: i64,
    pub prompt: i64,
    pub completion: i64,
    pub total: i64,
    pub calls: i64,
    pub compressions: Option<i64>,
    pub context_used: Option<i64>,
    pub context_max: Option<i64>,
    pub context_percent: Option<i64>,
    pub context_source: Option<String>,
    pub context_estimated: Option<bool>,
    pub cache_hit_pct: Option<i64>,
    pub cache_read: Option<i64>,
    pub cache_write: Option<i64>,
    pub avg_latency_s: Option<f64>,
    pub avg_tps: Option<f64>,
    pub active_subagents: Option<i64>,
    pub dev_credits_spent_micros: Option<i64>,
    pub cost_usd: Option<f64>,
    pub cost_status: Option<String>,
}

// ── REST: session listing ──────────────────────────────────────────────────────

/// One session row from `GET /api/sessions` (and the profile-wide lists).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SessionRow {
    pub id: String,
    pub title: Option<String>,
    pub source: Option<String>,
    pub model: Option<String>,
    pub started_at: f64,
    pub ended_at: Option<f64>,
    pub last_active: f64,
    pub message_count: i64,
    pub tool_call_count: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub preview: Option<String>,
    pub parent_session_id: Option<String>,
    /// The store keeps flags as 0/1; both spellings decode.
    #[serde(deserialize_with = "bool_from_any")]
    pub archived: bool,
    #[serde(deserialize_with = "bool_from_any")]
    pub pinned: bool,
    #[serde(default, deserialize_with = "opt_bool_from_any")]
    pub hidden: Option<bool>,
    #[serde(default, deserialize_with = "opt_bool_from_any")]
    pub unread: Option<bool>,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub git_repo_root: Option<String>,
    pub end_reason: Option<String>,
    pub profile: Option<String>,
    pub is_default_profile: Option<bool>,
    pub is_active: bool,
    pub scheduler_owned: Option<bool>,
    pub is_internal_child: Option<bool>,
    pub handoff_platform: Option<String>,
    pub handoff_state: Option<String>,
    pub handoff_error: Option<String>,
    pub actual_cost_usd: Option<f64>,
    pub estimated_cost_usd: Option<f64>,
}

/// A profile whose state.db could not be read while aggregating sessions.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ProfileError {
    pub profile: String,
    pub error: String,
}

/// `GET /api/sessions` and `GET /api/profiles/sessions` payload.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct PaginatedSessions {
    pub sessions: Vec<SessionRow>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
    /// Per-profile listable counts (`/api/profiles/sessions` only).
    pub profile_totals: HashMap<String, i64>,
    pub errors: Vec<ProfileError>,
    /// `{profile: "corrupt"}` for structurally damaged stores.
    pub storage: HashMap<String, String>,
}

/// Filters for `GET /api/sessions` / `GET /api/profiles/sessions`. Empty fields are omitted so
/// the backend defaults apply (limit 20; `archived=exclude`; `order=created` for `/api/sessions`,
/// `recent` and `profile=all` for the profiles route).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionListQuery {
    pub profile: String,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub min_messages: Option<u32>,
    /// `exclude` | `only` | `include`.
    pub archived: String,
    /// `created` | `recent`.
    pub order: String,
    pub source: String,
    pub sources: String,
    pub exclude_sources: String,
    pub cwd_prefix: String,
    pub full: bool,
}

fn session_list_path(base: &str, q: &SessionListQuery) -> String {
    let limit = q.limit.map(|n| n.to_string());
    let offset = q.offset.map(|n| n.to_string());
    let min_messages = q.min_messages.map(|n| n.to_string());
    let pairs: Vec<(&str, &str)> = vec![
        ("profile", q.profile.as_str()),
        ("limit", limit.as_deref().unwrap_or("")),
        ("offset", offset.as_deref().unwrap_or("")),
        ("min_messages", min_messages.as_deref().unwrap_or("")),
        ("archived", q.archived.as_str()),
        ("order", q.order.as_str()),
        ("source", q.source.as_str()),
        ("sources", q.sources.as_str()),
        ("exclude_sources", q.exclude_sources.as_str()),
        ("cwd_prefix", q.cwd_prefix.as_str()),
        ("full", if q.full { "1" } else { "" }),
    ];
    with_query(base, &pairs)
}

pub fn sessions_list_request(q: &SessionListQuery) -> Request {
    req(Method::Get, session_list_path("/api/sessions", q), None)
}

/// List stored sessions of one profile, newest-created by default.
pub async fn sessions_list(rest: &Rest, q: &SessionListQuery) -> Result<PaginatedSessions, ApiError> {
    send_as(rest, &sessions_list_request(q)).await
}

pub fn profiles_sessions_request(q: &SessionListQuery) -> Request {
    req(Method::Get, session_list_path("/api/profiles/sessions", q), None)
}

/// Unified session list aggregated across every profile (`profile` empty = all).
pub async fn profiles_sessions(rest: &Rest, q: &SessionListQuery) -> Result<PaginatedSessions, ApiError> {
    send_as(rest, &profiles_sessions_request(q)).await
}

// ── REST: search ───────────────────────────────────────────────────────────────

/// One search hit: an id/title/content match with lineage-deduped session metadata.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SessionSearchHit {
    pub session_id: String,
    pub lineage_root: Option<String>,
    pub id: String,
    pub title: Option<String>,
    pub source: Option<String>,
    pub model: Option<String>,
    pub started_at: Option<f64>,
    pub ended_at: Option<f64>,
    pub last_active: Option<f64>,
    pub is_active: bool,
    pub message_count: i64,
    pub tool_call_count: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub preview: Option<String>,
    pub parent_session_id: Option<String>,
    pub profile: Option<String>,
    pub is_default_profile: Option<bool>,
    #[serde(deserialize_with = "bool_from_any")]
    pub archived: bool,
    pub snippet: String,
    pub role: Option<String>,
    pub session_started: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SessionSearchResponse {
    pub results: Vec<SessionSearchHit>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionSearchQuery {
    pub q: String,
    pub limit: Option<u32>,
    pub profile: String,
    pub source: String,
    pub sources: String,
    pub exclude_sources: String,
}

pub fn sessions_search_request(q: &SessionSearchQuery) -> Request {
    let limit = q.limit.map(|n| n.to_string());
    let pairs: Vec<(&str, &str)> = vec![
        ("q", q.q.as_str()),
        ("limit", limit.as_deref().unwrap_or("")),
        ("profile", q.profile.as_str()),
        ("source", q.source.as_str()),
        ("sources", q.sources.as_str()),
        ("exclude_sources", q.exclude_sources.as_str()),
    ];
    req(Method::Get, with_query("/api/sessions/search", &pairs), None)
}

/// Search sessions by id, message content (FTS) and title.
pub async fn sessions_search(rest: &Rest, q: &SessionSearchQuery) -> Result<SessionSearchResponse, ApiError> {
    send_as(rest, &sessions_search_request(q)).await
}

// ── REST: one session, its messages and timeline paging ────────────────────────

/// One session's stored metadata (`GET /api/sessions/{id}`). Extra store columns are ignored.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SessionDetail {
    pub id: String,
    pub title: Option<String>,
    pub source: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub started_at: f64,
    pub ended_at: Option<f64>,
    pub last_active: f64,
    pub message_count: i64,
    pub tool_call_count: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub preview: Option<String>,
    pub parent_session_id: Option<String>,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub git_repo_root: Option<String>,
    pub end_reason: Option<String>,
    #[serde(deserialize_with = "bool_from_any")]
    pub archived: bool,
    #[serde(deserialize_with = "bool_from_any")]
    pub pinned: bool,
    #[serde(default, deserialize_with = "opt_bool_from_any")]
    pub hidden: Option<bool>,
    #[serde(default, deserialize_with = "opt_bool_from_any")]
    pub unread: Option<bool>,
    pub profile: Option<String>,
    pub is_default_profile: Option<bool>,
    pub scheduler_owned: Option<bool>,
    /// Optimistic-concurrency revision used by the REST mutation endpoints.
    pub runtime_revision: i64,
    pub runtime_generation: Option<i64>,
}

pub fn session_detail_request(session_id: &str, profile: &str) -> Request {
    let path = format!("/api/sessions/{}", seg(session_id));
    req(Method::Get, format!("{path}{}", profile_query(profile)), None)
}

/// Read one stored session (also answers 404 after a delete).
pub async fn session_detail(rest: &Rest, session_id: &str, profile: &str) -> Result<SessionDetail, ApiError> {
    send_as(rest, &session_detail_request(session_id, profile)).await
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionMessagesQuery {
    pub profile: String,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    /// `oldest` | `latest`; empty lets the backend pick (latest for an unpaged read).
    pub order: String,
    pub include_compacted: bool,
    /// Inline image parts in `content` (`false` asks for the `[image]` placeholder).
    pub include_inline_images: bool,
}

impl Default for SessionMessagesQuery {
    fn default() -> Self {
        Self {
            profile: String::new(),
            limit: None,
            offset: None,
            order: String::new(),
            include_compacted: false,
            include_inline_images: true,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct MessagesPagination {
    pub limit: i64,
    pub offset: i64,
    pub order: Option<String>,
    pub returned: i64,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SessionMessagesPage {
    pub session_id: String,
    pub profile: String,
    pub messages: Vec<Message>,
    pub pagination: MessagesPagination,
}

pub fn session_messages_request(session_id: &str, q: &SessionMessagesQuery) -> Request {
    let limit = q.limit.map(|n| n.to_string());
    let offset = q.offset.map(|n| n.to_string());
    let pairs: Vec<(&str, &str)> = vec![
        ("profile", q.profile.as_str()),
        ("limit", limit.as_deref().unwrap_or("")),
        ("offset", offset.as_deref().unwrap_or("")),
        ("order", q.order.as_str()),
        ("include_compacted", if q.include_compacted { "1" } else { "" }),
        ("inline_images", if q.include_inline_images { "" } else { "0" }),
    ];
    let path = format!("/api/sessions/{}/messages", seg(session_id));
    req(Method::Get, with_query(&path, &pairs), None)
}

/// Page a session's stored transcript (REST projection, same rows the transcript pane shows).
pub async fn session_messages(
    rest: &Rest,
    session_id: &str,
    q: &SessionMessagesQuery,
) -> Result<SessionMessagesPage, ApiError> {
    send_as(rest, &session_messages_request(session_id, q)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct AroundPagination {
    pub row_id: i64,
    pub limit: i64,
    pub returned: i64,
    pub order: String,
    pub offset: i64,
    pub total: i64,
    pub has_older: bool,
    pub has_newer: bool,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct MessagesAroundPage {
    pub session_id: String,
    pub profile: String,
    pub messages: Vec<Message>,
    pub pagination: AroundPagination,
}

pub fn session_messages_around_request(session_id: &str, row_id: i64, limit: Option<u32>, profile: &str) -> Request {
    let limit = limit.map(|n| n.to_string());
    let row = row_id.to_string();
    let pairs: Vec<(&str, &str)> = vec![
        ("row_id", row.as_str()),
        ("limit", limit.as_deref().unwrap_or("")),
        ("profile", profile),
    ];
    let path = format!("/api/sessions/{}/messages/around", seg(session_id));
    req(Method::Get, with_query(&path, &pairs), None)
}

/// Bounded display page starting at one prompt row (timeline jump).
pub async fn session_messages_around(
    rest: &Rest,
    session_id: &str,
    row_id: i64,
    limit: Option<u32>,
    profile: &str,
) -> Result<MessagesAroundPage, ApiError> {
    send_as(
        rest,
        &session_messages_around_request(session_id, row_id, limit, profile),
    )
    .await
}

// ── REST: sidebar slices ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SidebarQuery {
    /// `all` (default) or one profile name; scopes the whole payload.
    pub recents_profile: String,
    pub recents_limit: Option<u32>,
    pub recents_exclude: String,
    pub cron_limit: Option<u32>,
    pub messaging_limit: Option<u32>,
    pub messaging_exclude: String,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SidebarSlice {
    pub sessions: Vec<SessionRow>,
    pub profiles_truncated: HashMap<String, bool>,
    pub profiles_usage: HashMap<String, Value>,
    pub total: i64,
    pub profiles_failed: Vec<ProfileError>,
    pub errors: Vec<ProfileError>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SidebarPayload {
    pub recents: SidebarSlice,
    pub cron: SidebarSlice,
    pub messaging: SidebarSlice,
    pub errors: Vec<ProfileError>,
    pub storage: HashMap<String, String>,
    pub profiles_failed: Vec<ProfileError>,
}

pub fn profiles_sessions_sidebar_request(q: &SidebarQuery) -> Request {
    let recents_limit = q.recents_limit.map(|n| n.to_string());
    let cron_limit = q.cron_limit.map(|n| n.to_string());
    let messaging_limit = q.messaging_limit.map(|n| n.to_string());
    let pairs: Vec<(&str, &str)> = vec![
        ("recents_profile", q.recents_profile.as_str()),
        ("recents_limit", recents_limit.as_deref().unwrap_or("")),
        ("recents_exclude", q.recents_exclude.as_str()),
        ("cron_limit", cron_limit.as_deref().unwrap_or("")),
        ("messaging_limit", messaging_limit.as_deref().unwrap_or("")),
        ("messaging_exclude", q.messaging_exclude.as_str()),
    ];
    req(Method::Get, with_query("/api/profiles/sessions/sidebar", &pairs), None)
}

/// Batched sidebar slices (recents / cron / messaging) for every profile in one read.
pub async fn profiles_sessions_sidebar(rest: &Rest, q: &SidebarQuery) -> Result<SidebarPayload, ApiError> {
    send_as(rest, &profiles_sessions_sidebar_request(q)).await
}

// ── RPC: lifecycle and live state ──────────────────────────────────────────────

/// Approval prompt attached to an interrupted live turn.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct PendingApproval {
    pub request_id: Option<String>,
    pub command: Option<String>,
    pub description: Option<String>,
    pub pattern_key: Option<String>,
    pub pattern_keys: Option<Vec<String>>,
    pub allow_permanent: Option<bool>,
    pub allow_session: Option<bool>,
    pub smart_denied: Option<bool>,
    pub choices: Option<Vec<String>>,
    pub tool_name: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct InflightTurn {
    pub assistant: String,
    pub streaming: bool,
    pub user: String,
    pub display_kind: Option<String>,
    pub display_metadata: Option<Value>,
    pub corrections: Option<Vec<String>>,
    pub correction_offsets: Option<Vec<i64>>,
    pub error: Option<String>,
    pub status: Option<String>,
    pub recoverable: Option<bool>,
    pub error_surface: Option<Value>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct QueuedPrompt {
    pub user: String,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ActivateResult {
    pub session_id: String,
    pub message_count: i64,
    pub messages: Vec<Message>,
    pub info: LiveInfo,
    pub stored_session_id: Option<String>,
    pub resumed: Option<String>,
    pub session_key: Option<String>,
    pub messages_omitted: Option<bool>,
    pub hydrating: Option<bool>,
    pub running: Option<bool>,
    pub turn_started_at: Option<f64>,
    pub started_at: Option<f64>,
    pub status: Option<String>,
    pub inflight: Option<InflightTurn>,
    pub queued: Option<QueuedPrompt>,
    pub pending_approval: Option<PendingApproval>,
}

pub fn session_activate_call(session_id: &str, profile: &str, cols: Option<u32>, omit_messages: bool) -> Call {
    let mut p = json!({"session_id": session_id, "omit_messages": omit_messages});
    if let Some(c) = cols {
        p["cols"] = json!(c);
    }
    call_of("session.activate", profiled(p, profile))
}

/// Attach a stored session to the live runtime (optionally skipping the transcript).
pub async fn session_activate(
    client: &Client,
    session_id: &str,
    profile: &str,
    cols: Option<u32>,
    omit_messages: bool,
) -> Result<ActivateResult, ApiError> {
    call_as(client, &session_activate_call(session_id, profile, cols, omit_messages)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ActiveSession {
    pub current: bool,
    pub id: String,
    pub last_active: f64,
    pub message_count: i64,
    pub model: String,
    pub preview: String,
    pub session_key: String,
    pub started_at: f64,
    pub status: String,
    pub title: String,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ActiveListResult {
    pub sessions: Vec<ActiveSession>,
}

pub fn session_active_list_call(profile: &str, current_session_id: Option<&str>) -> Call {
    let mut p = json!({});
    if let Some(id) = current_session_id {
        p["current_session_id"] = json!(id);
    }
    call_of("session.active_list", profiled(p, profile))
}

/// Live sessions in this gateway process, newest-active first.
pub async fn session_active_list(
    client: &Client,
    profile: &str,
    current_session_id: Option<&str>,
) -> Result<ActiveListResult, ApiError> {
    call_as(client, &session_active_list_call(profile, current_session_id)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct MostRecent {
    pub session_id: Option<String>,
    pub title: Option<String>,
    pub started_at: Option<f64>,
    pub source: Option<String>,
}

pub fn session_most_recent_call(profile: &str) -> Call {
    call_of("session.most_recent", profiled(json!({}), profile))
}

/// Id/title/source of the profile's most recently started stored session.
pub async fn session_most_recent(client: &Client, profile: &str) -> Result<MostRecent, ApiError> {
    call_as(client, &session_most_recent_call(profile)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SessionStatus {
    /// Human-readable status block (may be empty for an idle session).
    pub output: String,
}

pub fn session_status_call(session_id: &str, profile: &str) -> Call {
    call_of("session.status", profiled(json!({"session_id": session_id}), profile))
}

/// Status text for one live session.
pub async fn session_status(client: &Client, session_id: &str, profile: &str) -> Result<SessionStatus, ApiError> {
    call_as(client, &session_status_call(session_id, profile)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct GoalSnapshot {
    pub title: String,
    pub status: String,
    pub turns_used: i64,
    pub max_turns: i64,
    pub contract: Value,
    pub subgoals: Vec<String>,
    pub gates: Vec<Value>,
    pub created_at: Option<f64>,
    pub updated_at: Option<f64>,
    pub paused_reason: Option<String>,
    pub last_verdict: Option<String>,
    pub last_reason: Option<String>,
    pub wait_barrier: Value,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct LoopSnapshot {
    pub prompt: String,
    pub status: String,
    pub mode: String,
    pub interval_seconds: f64,
    pub current_delay: f64,
    pub times: i64,
    pub until: String,
    pub max_ticks: i64,
    pub ticks_fired: i64,
    pub created_at: f64,
    pub last_fired_at: f64,
    pub next_due_at: f64,
    pub awaiting_response: bool,
    pub deferred_by_goal: bool,
    pub paused_reason: Option<String>,
    pub last_stop_reason: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct HeartbeatSnapshot {
    pub prompt: String,
    pub status: String,
    pub interval_seconds: i64,
    pub created_at: f64,
    pub last_fired_at: f64,
    pub fire_count: i64,
}

/// Structured goal / loop / heartbeat state of a live session.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ControlSnapshot {
    pub goal: Option<GoalSnapshot>,
    pub r#loop: Option<LoopSnapshot>,
    pub heartbeat: Option<HeartbeatSnapshot>,
    pub revision: String,
    pub updated_at: f64,
}

/// What applying a control action changed.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ControlDispatch {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub output: Option<String>,
    pub notice: Option<String>,
    pub message: Option<String>,
    pub display: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ControlRead {
    pub control: ControlSnapshot,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ControlResult {
    pub control: ControlSnapshot,
    pub dispatch: ControlDispatch,
}

pub fn session_control_read_call(session_id: &str, profile: &str) -> Call {
    call_of(
        "session.control.read",
        profiled(json!({"session_id": session_id}), profile),
    )
}

/// Read the session's goal / loop / heartbeat control state.
pub async fn session_control_read(client: &Client, session_id: &str, profile: &str) -> Result<ControlRead, ApiError> {
    call_as(client, &session_control_read_call(session_id, profile)).await
}

pub fn session_control_call(session_id: &str, profile: &str, action: &str, args: Option<Value>) -> Call {
    let mut p = json!({"session_id": session_id, "action": action});
    if let Some(a) = args {
        p["args"] = a;
    }
    call_of("session.control", profiled(p, profile))
}

/// Apply one control intent (`action` is a backend intent string, e.g. `heartbeat.clear`).
pub async fn session_control(
    client: &Client,
    session_id: &str,
    profile: &str,
    action: &str,
    args: Option<Value>,
) -> Result<ControlResult, ApiError> {
    call_as(client, &session_control_call(session_id, profile, action, args)).await
}

pub fn session_cwd_set_call(session_id: &str, profile: &str, cwd: &str) -> Call {
    call_of(
        "session.cwd.set",
        profiled(json!({"session_id": session_id, "cwd": cwd}), profile),
    )
}

/// Point the live session at a new working directory; the backend probes git there.
pub async fn session_cwd_set(
    client: &Client,
    session_id: &str,
    profile: &str,
    cwd: &str,
) -> Result<LiveInfo, ApiError> {
    call_as(client, &session_cwd_set_call(session_id, profile, cwd)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct WorkspaceMoved {
    pub cwd: String,
    pub branch: Option<String>,
    pub git_repo_root: Option<String>,
}

pub fn session_workspace_move_call(session_key: &str, profile: &str, cwd: &str) -> Call {
    call_of(
        "session.workspace.move",
        profiled(json!({"session_key": session_key, "cwd": cwd}), profile),
    )
}

/// Move a live session's workspace to `cwd` (same effect as `cwd.set`, keyed by session key).
pub async fn session_workspace_move(
    client: &Client,
    session_key: &str,
    profile: &str,
    cwd: &str,
) -> Result<WorkspaceMoved, ApiError> {
    call_as(client, &session_workspace_move_call(session_key, profile, cwd)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SessionClosed {
    pub closed: bool,
    /// One line per in-flight piece of work the close unwound.
    pub messages: Vec<String>,
}

pub fn session_close_call(session_id: &str, profile: &str) -> Call {
    call_of("session.close", profiled(json!({"session_id": session_id}), profile))
}

/// End the live runtime session (stored rows are untouched). **Destructive** to in-flight work.
pub async fn session_close(client: &Client, session_id: &str, profile: &str) -> Result<SessionClosed, ApiError> {
    call_as(client, &session_close_call(session_id, profile)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ArchiveState {
    pub archived: bool,
    pub session_key: String,
}

pub fn session_archive_call(session_id: &str, archived: bool, profile: &str) -> Call {
    call_of(
        "session.archive",
        profiled(json!({"session_id": session_id, "archived": archived}), profile),
    )
}

pub fn session_archive_key_call(session_key: &str, archived: bool, profile: &str) -> Call {
    call_of(
        "session.archive",
        profiled(json!({"session_key": session_key, "archived": archived}), profile),
    )
}

/// Archive or unarchive a stored session by id.
pub async fn session_archive(
    client: &Client,
    session_id: &str,
    archived: bool,
    profile: &str,
) -> Result<ArchiveState, ApiError> {
    call_as(client, &session_archive_call(session_id, archived, profile)).await
}

/// Archive or unarchive by session key (hosted-room key form of [`session_archive`]).
pub async fn session_archive_key(
    client: &Client,
    session_key: &str,
    archived: bool,
    profile: &str,
) -> Result<ArchiveState, ApiError> {
    call_as(client, &session_archive_key_call(session_key, archived, profile)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct HiddenState {
    pub hidden: bool,
    pub session_key: String,
}

pub fn session_set_hidden_call(session_id: &str, hidden: bool, profile: &str) -> Call {
    call_of(
        "session.set_hidden",
        profiled(json!({"session_id": session_id, "hidden": hidden}), profile),
    )
}

/// Hide or unhide a session in the sidebar (by stored id).
pub async fn session_set_hidden(
    client: &Client,
    session_id: &str,
    hidden: bool,
    profile: &str,
) -> Result<HiddenState, ApiError> {
    call_as(client, &session_set_hidden_call(session_id, hidden, profile)).await
}

// ── RPC: history, context and events ──────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct History {
    pub count: i64,
    pub messages: Vec<Message>,
}

pub fn session_history_call(session_id: &str, profile: &str) -> Call {
    call_of("session.history", profiled(json!({"session_id": session_id}), profile))
}

/// Full stored transcript of a session (gateway projection).
pub async fn session_history(client: &Client, session_id: &str, profile: &str) -> Result<History, ApiError> {
    call_as(client, &session_history_call(session_id, profile)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ContextCategory {
    pub color: String,
    pub id: String,
    pub label: String,
    pub tokens: i64,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ContextFile {
    pub label: String,
    pub path: String,
    pub chars: i64,
    pub est_tokens: i64,
    pub loaded: bool,
    pub status: String,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ContextBreakdown {
    pub categories: Vec<ContextCategory>,
    pub context_max: i64,
    pub context_percent: i64,
    pub context_used: i64,
    pub estimated_total: i64,
    pub context_estimated: bool,
    pub context_source: String,
    pub model: String,
    pub context_files: Vec<ContextFile>,
}

pub fn session_context_breakdown_call(session_id: &str, profile: &str) -> Call {
    call_of(
        "session.context_breakdown",
        profiled(json!({"session_id": session_id}), profile),
    )
}

/// Per-category token breakdown of the session's context window.
pub async fn session_context_breakdown(
    client: &Client,
    session_id: &str,
    profile: &str,
) -> Result<ContextBreakdown, ApiError> {
    call_as(client, &session_context_breakdown_call(session_id, profile)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct EventsSince {
    /// Event objects are free-form (`{type, seq, ...}`); only the envelope is typed.
    pub events: Vec<Value>,
    pub latest_seq: i64,
    pub truncated: bool,
    pub count: i64,
    pub epoch: String,
    pub open_requests: Vec<Value>,
}

pub fn session_events_since_call(session_id: &str, profile: &str, last_seen: Option<i64>) -> Call {
    let mut p = json!({"session_id": session_id});
    if let Some(seq) = last_seen {
        p["last_seen"] = json!(seq);
    }
    call_of("session.events.since", profiled(p, profile))
}

/// Replay a session's event ring after `last_seen` (WS reconnect).
pub async fn session_events_since(
    client: &Client,
    session_id: &str,
    profile: &str,
    last_seen: Option<i64>,
) -> Result<EventsSince, ApiError> {
    call_as(client, &session_events_since_call(session_id, profile, last_seen)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct EventsStats {
    pub sessions: i64,
    pub events: i64,
    pub bytes: i64,
    pub max_per_session: i64,
    pub max_bytes_per_session: i64,
    pub max_bytes_process: i64,
}

pub fn session_events_stats_call(profile: &str) -> Call {
    call_of("session.events.stats", profiled(json!({}), profile))
}

/// Event-ring statistics for the gateway process.
pub async fn session_events_stats(client: &Client, profile: &str) -> Result<EventsStats, ApiError> {
    call_as(client, &session_events_stats_call(profile)).await
}

// ── RPC: foreign sessions ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ForeignSession {
    pub id: String,
    /// Backend enum (`ForeignSource`), e.g. another host's transcript store.
    pub source: String,
    pub label: String,
    pub title: String,
    pub cwd: Option<String>,
    pub mtime: f64,
    pub turn_count: i64,
    pub excerpt: String,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ForeignList {
    pub sessions: Vec<ForeignSession>,
    pub next_offset: Option<i64>,
    pub host: String,
    pub unreadable: i64,
}

pub fn session_foreign_list_call(profile: &str, source: Option<&str>, offset: Option<i64>, limit: Option<i64>) -> Call {
    let mut p = json!({});
    if let Some(s) = source {
        p["source"] = json!(s);
    }
    if let Some(o) = offset {
        p["offset"] = json!(o);
    }
    if let Some(l) = limit {
        p["limit"] = json!(l);
    }
    call_of("session.foreign.list", profiled(p, profile))
}

/// List sessions this host can import from other backends/hosts.
pub async fn session_foreign_list(
    client: &Client,
    profile: &str,
    source: Option<&str>,
    offset: Option<i64>,
    limit: Option<i64>,
) -> Result<ForeignList, ApiError> {
    call_as(client, &session_foreign_list_call(profile, source, offset, limit)).await
}

// ── RPC: branching, saving, rewinding, compression ────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct BranchStored {
    pub session_id: String,
    pub stored_session_id: String,
    pub message_count: i64,
    pub messages_omitted: bool,
    pub info: LiveInfo,
}

pub fn session_branch_stored_call(
    parent_session_id: &str,
    profile: &str,
    cols: Option<u32>,
    source: Option<&str>,
    cwd: Option<&str>,
    idempotency_key: Option<&str>,
) -> Call {
    let mut p = json!({"parent_session_id": parent_session_id});
    if let Some(c) = cols {
        p["cols"] = json!(c);
    }
    if let Some(s) = source {
        p["source"] = json!(s);
    }
    if let Some(c) = cwd {
        p["cwd"] = json!(c);
    }
    if let Some(k) = idempotency_key {
        p["idempotency_key"] = json!(k);
    }
    call_of("session.branch_stored", profiled(p, profile))
}

/// Branch a session from its stored transcript without re-running turns.
pub async fn session_branch_stored(
    client: &Client,
    parent_session_id: &str,
    profile: &str,
    cols: Option<u32>,
    source: Option<&str>,
    cwd: Option<&str>,
    idempotency_key: Option<&str>,
) -> Result<BranchStored, ApiError> {
    call_as(
        client,
        &session_branch_stored_call(parent_session_id, profile, cols, source, cwd, idempotency_key),
    )
    .await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct Branch {
    pub session_id: String,
    pub stored_session_id: String,
    pub title: String,
    pub parent: String,
    pub message_count: i64,
    pub messages: Vec<Message>,
    pub info: LiveInfo,
}

pub fn session_branch_call(
    session_id: &str,
    profile: &str,
    name: Option<&str>,
    count: Option<u32>,
    idempotency_key: Option<&str>,
) -> Call {
    let mut p = json!({"session_id": session_id});
    if let Some(n) = name {
        p["name"] = json!(n);
    }
    if let Some(c) = count {
        p["count"] = json!(c);
    }
    if let Some(k) = idempotency_key {
        p["idempotency_key"] = json!(k);
    }
    call_of("session.branch", profiled(p, profile))
}

/// Branch the last `count` turns, replaying them in a fresh session. **Spends model calls.**
pub async fn session_branch(
    client: &Client,
    session_id: &str,
    profile: &str,
    name: Option<&str>,
    count: Option<u32>,
    idempotency_key: Option<&str>,
) -> Result<Branch, ApiError> {
    call_as(
        client,
        &session_branch_call(session_id, profile, name, count, idempotency_key),
    )
    .await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct BranchWhole {
    pub session_id: String,
    pub stored_session_id: String,
    pub title: String,
    pub parent: String,
    pub message_count: i64,
    pub messages_omitted: bool,
    pub info: LiveInfo,
}

pub fn session_branch_whole_call(
    session_id: &str,
    profile: &str,
    name: Option<&str>,
    idempotency_key: Option<&str>,
) -> Call {
    let mut p = json!({"session_id": session_id});
    if let Some(n) = name {
        p["name"] = json!(n);
    }
    if let Some(k) = idempotency_key {
        p["idempotency_key"] = json!(k);
    }
    call_of("session.branch_whole", profiled(p, profile))
}

/// Fork the whole conversation into a new session. **Spends model calls.**
pub async fn session_branch_whole(
    client: &Client,
    session_id: &str,
    profile: &str,
    name: Option<&str>,
    idempotency_key: Option<&str>,
) -> Result<BranchWhole, ApiError> {
    call_as(
        client,
        &session_branch_whole_call(session_id, profile, name, idempotency_key),
    )
    .await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SavedSession {
    pub file: Option<String>,
}

pub fn session_save_call(session_id: &str, profile: &str) -> Call {
    call_of("session.save", profiled(json!({"session_id": session_id}), profile))
}

/// Persist the live session to its `session_<id>.json` snapshot on disk.
pub async fn session_save(client: &Client, session_id: &str, profile: &str) -> Result<SavedSession, ApiError> {
    call_as(client, &session_save_call(session_id, profile)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct UndoResult {
    pub removed: i64,
}

pub fn session_undo_call(session_id: &str, profile: &str, intent: Option<&str>) -> Call {
    let mut p = json!({"session_id": session_id});
    if let Some(i) = intent {
        p["intent"] = json!(i);
    }
    call_of("session.undo", profiled(p, profile))
}

/// Rewind the last turn (`intent` is the backend's undo-intent enum). **Destructive** to rows.
pub async fn session_undo(
    client: &Client,
    session_id: &str,
    profile: &str,
    intent: Option<&str>,
) -> Result<UndoResult, ApiError> {
    call_as(client, &session_undo_call(session_id, profile, intent)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct CompressionSummary {
    pub noop: bool,
    pub aborted: bool,
    pub refused_would_grow: Option<bool>,
    pub fallback_used: Option<bool>,
    pub headline: String,
    pub token_line: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct CompressResult {
    pub status: Option<String>,
    pub removed: Option<i64>,
    pub before_messages: Option<i64>,
    pub after_messages: Option<i64>,
    pub before_tokens: Option<i64>,
    pub after_tokens: Option<i64>,
    pub summary: Option<CompressionSummary>,
    pub usage: Option<Usage>,
    pub info: Option<LiveInfo>,
    pub messages: Option<Vec<Message>>,
    pub compressed: Option<bool>,
    pub lock_held: Option<bool>,
    pub message: Option<String>,
    pub turn_isolation: Option<bool>,
    pub host_ack: Value,
}

pub fn session_compress_call(session_id: &str, profile: &str, focus_topic: Option<&str>) -> Call {
    let mut p = json!({"session_id": session_id});
    if let Some(t) = focus_topic {
        p["focus_topic"] = json!(t);
    }
    call_of("session.compress", profiled(p, profile))
}

/// Compress the transcript with a model summary. **Spends model calls.**
pub async fn session_compress(
    client: &Client,
    session_id: &str,
    profile: &str,
    focus_topic: Option<&str>,
) -> Result<CompressResult, ApiError> {
    call_as(client, &session_compress_call(session_id, profile, focus_topic)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct Correction {
    pub status: String,
    pub text: String,
}

pub fn session_steer_call(session_id: &str, profile: &str, text: &str) -> Call {
    call_of(
        "session.steer",
        profiled(json!({"session_id": session_id, "text": text}), profile),
    )
}

/// Add mid-turn guidance to the running turn. **Spends model calls.**
pub async fn session_steer(
    client: &Client,
    session_id: &str,
    profile: &str,
    text: &str,
) -> Result<Correction, ApiError> {
    call_as(client, &session_steer_call(session_id, profile, text)).await
}

pub fn session_redirect_call(session_id: &str, profile: &str, text: &str) -> Call {
    call_of(
        "session.redirect",
        profiled(json!({"session_id": session_id, "text": text}), profile),
    )
}

/// Replace the running turn's direction. **Spends model calls.**
pub async fn session_redirect(
    client: &Client,
    session_id: &str,
    profile: &str,
    text: &str,
) -> Result<Correction, ApiError> {
    call_as(client, &session_redirect_call(session_id, profile, text)).await
}

pub fn session_usage_call(session_id: &str, profile: &str) -> Call {
    call_of("session.usage", profiled(json!({"session_id": session_id}), profile))
}

/// Token/cost usage for one session. **Network** (provider limits / Nous credits).
pub async fn session_usage(client: &Client, session_id: &str, profile: &str) -> Result<Usage, ApiError> {
    call_as(client, &session_usage_call(session_id, profile)).await
}

// ── RPC: approvals and request answers ────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ApprovalList {
    pub approvals: Vec<PendingApproval>,
}

pub fn approval_pending_call(session_id: &str, profile: &str) -> Call {
    call_of("approval.pending", profiled(json!({"session_id": session_id}), profile))
}

/// Approvals waiting on the user for a live session.
pub async fn approval_pending(client: &Client, session_id: &str, profile: &str) -> Result<ApprovalList, ApiError> {
    call_as(client, &approval_pending_call(session_id, profile)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ApprovalReceived {
    pub acknowledged: bool,
}

pub fn approval_received_call(session_id: &str, request_id: &str, profile: &str) -> Call {
    call_of(
        "approval.received",
        profiled(json!({"session_id": session_id, "request_id": request_id}), profile),
    )
}

/// Mark one approval prompt as delivered/seen.
pub async fn approval_received(
    client: &Client,
    session_id: &str,
    request_id: &str,
    profile: &str,
) -> Result<ApprovalReceived, ApiError> {
    call_as(client, &approval_received_call(session_id, request_id, profile)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct AnswerStatus {
    pub status: String,
}

pub fn request_answer_call(id: &str, result: Value, profile: &str) -> Call {
    call_of("request.answer", profiled(json!({"id": id, "result": result}), profile))
}

/// Answer a pending `request` (clarify/question) by id. A matching request resumes its turn.
pub async fn request_answer(client: &Client, id: &str, result: Value, profile: &str) -> Result<AnswerStatus, ApiError> {
    call_as(client, &request_answer_call(id, result, profile)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ClarifyLock {
    pub status: String,
    pub remaining: Option<Vec<String>>,
}

pub fn clarify_lock_call(request_id: &str, question_id: &str, answer: Option<Value>, profile: &str) -> Call {
    let mut p = json!({"request_id": request_id, "question_id": question_id});
    if let Some(a) = answer {
        p["answer"] = a;
    }
    call_of("clarify.lock", profiled(p, profile))
}

/// Lock one clarify question's answer before the request is answered (idempotent).
pub async fn clarify_lock(
    client: &Client,
    request_id: &str,
    question_id: &str,
    answer: Option<Value>,
    profile: &str,
) -> Result<ClarifyLock, ApiError> {
    call_as(client, &clarify_lock_call(request_id, question_id, answer, profile)).await
}

// ── RPC: input completions and commands ───────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct CompletionItem {
    pub text: String,
    pub display: String,
    pub meta: String,
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct CompletionItems {
    pub items: Vec<CompletionItem>,
}

pub fn complete_path_call(word: Option<&str>, cwd: Option<&str>, session_id: Option<&str>, profile: &str) -> Call {
    let mut p = json!({});
    if let Some(w) = word {
        p["word"] = json!(w);
    }
    if let Some(c) = cwd {
        p["cwd"] = json!(c);
    }
    if let Some(s) = session_id {
        p["session_id"] = json!(s);
    }
    call_of("complete.path", profiled(p, profile))
}

/// Filesystem completion for an `@path` token.
pub async fn complete_path(
    client: &Client,
    word: Option<&str>,
    cwd: Option<&str>,
    session_id: Option<&str>,
    profile: &str,
) -> Result<CompletionItems, ApiError> {
    call_as(client, &complete_path_call(word, cwd, session_id, profile)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SlashCompletions {
    pub items: Vec<CompletionItem>,
    pub replace_from: Option<i64>,
}

pub fn complete_slash_call(text: Option<&str>, session_id: Option<&str>, profile: &str) -> Call {
    let mut p = json!({});
    if let Some(t) = text {
        p["text"] = json!(t);
    }
    if let Some(s) = session_id {
        p["session_id"] = json!(s);
    }
    call_of("complete.slash", profiled(p, profile))
}

/// Slash-command / skill completion for the composer.
pub async fn complete_slash(
    client: &Client,
    text: Option<&str>,
    session_id: Option<&str>,
    profile: &str,
) -> Result<SlashCompletions, ApiError> {
    call_as(client, &complete_slash_call(text, session_id, profile)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct DetectedDrop {
    pub name: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub token_estimate: Option<i64>,
    pub matched: bool,
    pub is_image: Option<bool>,
    pub path: Option<String>,
    pub count: Option<i64>,
    pub text: Option<String>,
}

pub fn input_detect_drop_call(session_id: &str, text: Option<&str>, profile: &str) -> Call {
    let mut p = json!({"session_id": session_id});
    if let Some(t) = text {
        p["text"] = json!(t);
    }
    call_of("input.detect_drop", profiled(p, profile))
}

/// Detect a dragged/dropped file reference in composer text.
pub async fn input_detect_drop(
    client: &Client,
    session_id: &str,
    text: Option<&str>,
    profile: &str,
) -> Result<DetectedDrop, ApiError> {
    call_as(client, &input_detect_drop_call(session_id, text, profile)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct CollapsedPaste {
    pub placeholder: String,
    pub path: String,
    pub lines: i64,
}

pub fn paste_collapse_call(text: Option<&str>) -> Call {
    let mut p = json!({});
    if let Some(t) = text {
        p["text"] = json!(t);
    }
    call_of("paste.collapse", p)
}

/// Collapse a large paste into a placeholder + on-disk path.
pub async fn paste_collapse(client: &Client, text: Option<&str>) -> Result<CollapsedPaste, ApiError> {
    call_as(client, &paste_collapse_call(text)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct CommandCategory {
    pub name: String,
    /// `[[key, description], ...]` pairs.
    pub pairs: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct CommandCatalog {
    pub pairs: Vec<Vec<String>>,
    pub sub: Value,
    pub canon: Value,
    pub commands: Value,
    pub categories: Vec<CommandCategory>,
    pub skills: Value,
    pub skill_count: i64,
    pub warning: String,
}

pub fn commands_catalog_call(session_id: Option<&str>, profile: &str) -> Call {
    let mut p = json!({});
    if let Some(s) = session_id {
        p["session_id"] = json!(s);
    }
    call_of("commands.catalog", profiled(p, profile))
}

/// Every slash command / skill the composer can offer.
pub async fn commands_catalog(
    client: &Client,
    session_id: Option<&str>,
    profile: &str,
) -> Result<CommandCatalog, ApiError> {
    call_as(client, &commands_catalog_call(session_id, profile)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ResolvedCommand {
    pub canonical: String,
    pub description: String,
    pub category: String,
}

pub fn command_resolve_call(name: Option<&str>, profile: &str) -> Call {
    let mut p = json!({});
    if let Some(n) = name {
        p["name"] = json!(n);
    }
    call_of("command.resolve", profiled(p, profile))
}

/// Resolve one command/skill name to its canonical form, description and category.
pub async fn command_resolve(client: &Client, name: Option<&str>, profile: &str) -> Result<ResolvedCommand, ApiError> {
    call_as(client, &command_resolve_call(name, profile)).await
}

// ── RPC: side agents and reactions ───────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct TaskId {
    pub task_id: String,
}

pub fn prompt_background_call(session_id: &str, profile: &str, text: &str) -> Call {
    call_of(
        "prompt.background",
        profiled(json!({"session_id": session_id, "text": text}), profile),
    )
}

/// Run the prompt as a background side agent. **Spends model calls.**
pub async fn prompt_background(
    client: &Client,
    session_id: &str,
    profile: &str,
    text: &str,
) -> Result<TaskId, ApiError> {
    call_as(client, &prompt_background_call(session_id, profile, text)).await
}

pub fn prompt_btw_call(session_id: &str, profile: &str, text: &str) -> Call {
    call_of(
        "prompt.btw",
        profiled(json!({"session_id": session_id, "text": text}), profile),
    )
}

/// Ask a side question without derailing the main turn. **Spends model calls.**
pub async fn prompt_btw(client: &Client, session_id: &str, profile: &str, text: &str) -> Result<TaskId, ApiError> {
    call_as(client, &prompt_btw_call(session_id, profile, text)).await
}

/// `preview.restart`: hand the preview pane's failure to a hidden side agent that diagnoses
/// the URL and — deliberately — leaves a restarted dev server running under its task id.
pub fn preview_restart_call(session_id: &str, url: &str, cwd: &str, context: &str, profile: &str) -> Call {
    let mut p = json!({"session_id": session_id, "url": url});
    if !cwd.is_empty() {
        p["cwd"] = json!(cwd);
    }
    if !context.is_empty() {
        p["context"] = json!(context);
    }
    call_of("preview.restart", profiled(p, profile))
}

/// Restart the preview dev server. **Spawns a side agent / spends model calls** — builder +
/// unit test only.
pub async fn preview_restart(
    client: &Client,
    session_id: &str,
    url: &str,
    cwd: &str,
    context: &str,
    profile: &str,
) -> Result<TaskId, ApiError> {
    call_as(client, &preview_restart_call(session_id, url, cwd, context, profile)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct MessageReaction {
    pub emoji: String,
    /// `user` or `agent`.
    pub author: String,
    pub at: Option<f64>,
    pub seen: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct Reactions {
    pub row_id: i64,
    pub reactions: Vec<MessageReaction>,
}

pub fn message_react_call(
    session_id: &str,
    profile: &str,
    row_id: Option<i64>,
    newest_role: Option<&str>,
    emoji: Option<&str>,
    author: Option<&str>,
) -> Call {
    let mut p = json!({"session_id": session_id});
    if let Some(r) = row_id {
        p["row_id"] = json!(r);
    }
    if let Some(r) = newest_role {
        p["newest_role"] = json!(r);
    }
    p["emoji"] = emoji.map_or(Value::Null, |e| json!(e));
    if let Some(a) = author {
        p["author"] = json!(a);
    }
    call_of("message.react", profiled(p, profile))
}

/// Set (`emoji`) or clear (`emoji: None`) this author's reaction on one message row.
pub async fn message_react(
    client: &Client,
    session_id: &str,
    profile: &str,
    row_id: Option<i64>,
    newest_role: Option<&str>,
    emoji: Option<&str>,
    author: Option<&str>,
) -> Result<Reactions, ApiError> {
    call_as(
        client,
        &message_react_call(session_id, profile, row_id, newest_role, emoji, author),
    )
    .await
}

// ── RPC: one-shot LLM and image generation ───────────────────────────────────

/// `llm.oneshot`: aux-model text (titles, project ideas).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct LlmOneshot {
    pub text: String,
}

/// The optional `llm.oneshot` knobs (backend defaults: `task: "title_generation"`,
/// `max_tokens: 1024`, `temperature: 0.3`).
#[derive(Debug, Clone, Default)]
pub struct LlmOneshotOptions<'a> {
    /// The template's substitution object.
    pub variables: Option<&'a Value>,
    pub task: &'a str,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
}

/// `llm.oneshot`: a stateless one-shot. A live `session_id` lends its model, else the `options`
/// task backend runs it. At least a template or `instructions`/`input` is required.
///
/// **Spends model calls.**
pub fn llm_oneshot_call(
    session_id: &str,
    template: &str,
    instructions: &str,
    input: &str,
    options: &LlmOneshotOptions,
    profile: &str,
) -> Call {
    let mut p = json!({});
    if !session_id.is_empty() {
        p["session_id"] = json!(session_id);
    }
    if !template.is_empty() {
        p["template"] = json!(template);
    }
    if !instructions.is_empty() {
        p["instructions"] = json!(instructions);
    }
    if !input.is_empty() {
        p["input"] = json!(input);
    }
    if let Some(v) = options.variables {
        p["variables"] = v.clone();
    }
    if !options.task.is_empty() {
        p["task"] = json!(options.task);
    }
    if let Some(v) = options.max_tokens {
        p["max_tokens"] = json!(v);
    }
    if let Some(v) = options.temperature {
        p["temperature"] = json!(v);
    }
    call_of("llm.oneshot", profiled(p, profile))
}

/// Run a one-shot aux generation. **Spends model calls.**
pub async fn llm_oneshot(
    client: &Client,
    session_id: &str,
    template: &str,
    instructions: &str,
    input: &str,
    options: &LlmOneshotOptions<'_>,
    profile: &str,
) -> Result<LlmOneshot, ApiError> {
    call_as(
        client,
        &llm_oneshot_call(session_id, template, instructions, input, options, profile),
    )
    .await
}

/// `image.generate` reply; `image_data` (a `data:` URL) is omitted when the download failed —
/// fall back to `image` (the backend's URL/path).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ImageGenerated {
    /// Provider availability; `probe: true` answers this alone.
    pub available: bool,
    pub success: bool,
    pub image: String,
    pub image_data: Option<String>,
    pub error: Option<String>,
}

/// `image.generate`: `prompt` (required unless `probe`), `aspect_ratio`
/// (`landscape` | `square` | `portrait`, `square` default) and `max_bytes` (cap on the data
/// URL; backend default 8 MB, max 16 MB). `probe: true` answers availability only.
///
/// **Spends model calls / uses the network** — calls the image provider (a `probe` does not).
pub fn image_generate_call(prompt: &str, aspect_ratio: &str, probe: bool, max_bytes: Option<u64>) -> Call {
    let mut p = json!({});
    if !prompt.is_empty() {
        p["prompt"] = json!(prompt);
    }
    if !aspect_ratio.is_empty() {
        p["aspect_ratio"] = json!(aspect_ratio);
    }
    if probe {
        p["probe"] = json!(true);
    }
    if let Some(v) = max_bytes {
        p["max_bytes"] = json!(v);
    }
    call_of("image.generate", p)
}

/// Generate one image. **Spends model calls / uses the network.**
pub async fn image_generate(
    client: &Client,
    prompt: &str,
    aspect_ratio: &str,
    probe: bool,
    max_bytes: Option<u64>,
) -> Result<ImageGenerated, ApiError> {
    call_as(client, &image_generate_call(prompt, aspect_ratio, probe, max_bytes)).await
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{Call, decode};
    use crate::http::Method;
    use serde_json::{Value, json};

    fn assert_call(c: &Call, method: &str, params: Value) {
        assert_eq!(c.method, method);
        assert_eq!(c.params, params);
    }

    #[test]
    fn rest_list_builders() {
        let r = sessions_list_request(&SessionListQuery::default());
        assert_eq!(
            (r.method, r.path.as_str(), r.body),
            (Method::Get, "/api/sessions", None)
        );

        let q = SessionListQuery {
            profile: "swe".into(),
            limit: Some(10),
            offset: Some(5),
            min_messages: Some(2),
            archived: "include".into(),
            order: "recent".into(),
            source: "cron".into(),
            sources: "desktop,tui".into(),
            exclude_sources: "telegram".into(),
            cwd_prefix: "tmp dir".into(),
            full: true,
        };
        assert_eq!(
            sessions_list_request(&q).path,
            "/api/sessions?profile=swe&limit=10&offset=5&min_messages=2&archived=include&order=recent\
             &source=cron&sources=desktop%2Ctui&exclude_sources=telegram&cwd_prefix=tmp%20dir&full=1"
        );

        assert_eq!(
            profiles_sessions_request(&SessionListQuery::default()).path,
            "/api/profiles/sessions"
        );
        assert_eq!(sessions_list_request(&q).method, Method::Get);

        let s = profiles_sessions_sidebar_request(&SidebarQuery {
            recents_profile: "all".into(),
            recents_limit: Some(20),
            recents_exclude: "cron".into(),
            ..SidebarQuery::default()
        });
        assert_eq!(s.method, Method::Get);
        assert_eq!(
            s.path,
            "/api/profiles/sessions/sidebar?recents_profile=all&recents_limit=20&recents_exclude=cron"
        );
    }

    #[test]
    fn rest_session_builders() {
        let d = session_detail_request("a b", "swe");
        assert_eq!(d.method, Method::Get);
        assert_eq!(d.path, "/api/sessions/a%20b?profile=swe");

        let m = session_messages_request(
            "s1",
            &SessionMessagesQuery {
                limit: Some(100),
                offset: Some(50),
                order: "latest".into(),
                include_compacted: true,
                ..SessionMessagesQuery::default()
            },
        );
        assert_eq!(
            m.path,
            "/api/sessions/s1/messages?limit=100&offset=50&order=latest&include_compacted=1"
        );
        assert_eq!(
            session_messages_request("s1", &SessionMessagesQuery::default()).path,
            "/api/sessions/s1/messages"
        );
        assert_eq!(
            session_messages_request(
                "s1",
                &SessionMessagesQuery {
                    include_inline_images: false,
                    ..SessionMessagesQuery::default()
                }
            )
            .path,
            "/api/sessions/s1/messages?inline_images=0"
        );

        let a = session_messages_around_request("s1", 42, Some(120), "");
        assert_eq!(a.path, "/api/sessions/s1/messages/around?row_id=42&limit=120");
        assert_eq!(
            session_messages_around_request("s1", 7, None, "default").path,
            "/api/sessions/s1/messages/around?row_id=7&profile=default"
        );

        let se = sessions_search_request(&SessionSearchQuery {
            q: "hello world".into(),
            limit: Some(5),
            profile: "swe".into(),
            ..SessionSearchQuery::default()
        });
        assert_eq!(se.path, "/api/sessions/search?q=hello%20world&limit=5&profile=swe");
    }

    #[test]
    fn rpc_lifecycle_builders() {
        assert_call(
            &session_activate_call("s1", "", Some(120), true),
            "session.activate",
            json!({"session_id": "s1", "cols": 120, "omit_messages": true}),
        );
        assert_call(
            &session_activate_call("s1", "swe", None, false),
            "session.activate",
            json!({"session_id": "s1", "profile": "swe", "omit_messages": false}),
        );
        assert_call(
            &session_active_list_call("", Some("s1")),
            "session.active_list",
            json!({"current_session_id": "s1"}),
        );
        assert_call(
            &session_active_list_call("swe", None),
            "session.active_list",
            json!({"profile": "swe"}),
        );
        assert_call(&session_most_recent_call(""), "session.most_recent", json!({}));
        assert_call(
            &session_status_call("s1", ""),
            "session.status",
            json!({"session_id": "s1"}),
        );
        assert_call(
            &session_control_read_call("s1", "swe"),
            "session.control.read",
            json!({"session_id": "s1", "profile": "swe"}),
        );
        assert_call(
            &session_control_call("s1", "", "heartbeat.clear", None),
            "session.control",
            json!({"session_id": "s1", "action": "heartbeat.clear"}),
        );
        assert_call(
            &session_control_call("s1", "", "loop.pause", Some(json!({"index": 1}))),
            "session.control",
            json!({"session_id": "s1", "action": "loop.pause", "args": {"index": 1}}),
        );
        assert_call(
            &session_cwd_set_call("s1", "", "/tmp"),
            "session.cwd.set",
            json!({"session_id": "s1", "cwd": "/tmp"}),
        );
        assert_call(
            &session_workspace_move_call("k1", "swe", "/tmp"),
            "session.workspace.move",
            json!({"session_key": "k1", "profile": "swe", "cwd": "/tmp"}),
        );
        assert_call(
            &session_close_call("s1", ""),
            "session.close",
            json!({"session_id": "s1"}),
        );
        assert_call(
            &session_archive_call("s1", true, ""),
            "session.archive",
            json!({"session_id": "s1", "archived": true}),
        );
        assert_call(
            &session_archive_key_call("k1", false, "swe"),
            "session.archive",
            json!({"session_key": "k1", "archived": false, "profile": "swe"}),
        );
        assert_call(
            &session_set_hidden_call("s1", true, ""),
            "session.set_hidden",
            json!({"session_id": "s1", "hidden": true}),
        );
    }

    #[test]
    fn rpc_history_builders() {
        assert_call(
            &session_history_call("s1", ""),
            "session.history",
            json!({"session_id": "s1"}),
        );
        assert_call(
            &session_context_breakdown_call("s1", "swe"),
            "session.context_breakdown",
            json!({"session_id": "s1", "profile": "swe"}),
        );
        assert_call(
            &session_events_since_call("s1", "", Some(9)),
            "session.events.since",
            json!({"session_id": "s1", "last_seen": 9}),
        );
        assert_call(
            &session_events_since_call("s1", "", None),
            "session.events.since",
            json!({"session_id": "s1"}),
        );
        assert_call(&session_events_stats_call(""), "session.events.stats", json!({}));
        assert_call(
            &session_foreign_list_call("", Some("codex"), Some(2), Some(5)),
            "session.foreign.list",
            json!({"source": "codex", "offset": 2, "limit": 5}),
        );
        assert_call(
            &session_foreign_list_call("swe", None, None, None),
            "session.foreign.list",
            json!({"profile": "swe"}),
        );
    }

    #[test]
    fn rpc_branch_and_side_agent_builders() {
        assert_call(
            &session_branch_stored_call("p1", "", Some(80), Some("desktop"), Some("/tmp"), Some("k")),
            "session.branch_stored",
            json!({"parent_session_id": "p1", "cols": 80, "source": "desktop", "cwd": "/tmp", "idempotency_key": "k"}),
        );
        assert_call(
            &session_branch_call("s1", "", Some("Topic"), Some(3), Some("k2")),
            "session.branch",
            json!({"session_id": "s1", "name": "Topic", "count": 3, "idempotency_key": "k2"}),
        );
        assert_call(
            &session_branch_whole_call("s1", "swe", Some("Topic"), None),
            "session.branch_whole",
            json!({"session_id": "s1", "profile": "swe", "name": "Topic"}),
        );
        assert_call(
            &session_save_call("s1", ""),
            "session.save",
            json!({"session_id": "s1"}),
        );
        assert_call(
            &session_undo_call("s1", "", Some("last_turn")),
            "session.undo",
            json!({"session_id": "s1", "intent": "last_turn"}),
        );
        assert_call(
            &session_compress_call("s1", "", Some("decisions")),
            "session.compress",
            json!({"session_id": "s1", "focus_topic": "decisions"}),
        );
        assert_call(
            &session_steer_call("s1", "", "focus on tests"),
            "session.steer",
            json!({"session_id": "s1", "text": "focus on tests"}),
        );
        assert_call(
            &session_redirect_call("s1", "swe", "other direction"),
            "session.redirect",
            json!({"session_id": "s1", "profile": "swe", "text": "other direction"}),
        );
        assert_call(
            &session_usage_call("s1", ""),
            "session.usage",
            json!({"session_id": "s1"}),
        );
        assert_call(
            &approval_pending_call("s1", ""),
            "approval.pending",
            json!({"session_id": "s1"}),
        );
        assert_call(
            &approval_received_call("s1", "r1", ""),
            "approval.received",
            json!({"session_id": "s1", "request_id": "r1"}),
        );
        assert_call(
            &request_answer_call("q1", json!({"choice": "yes"}), ""),
            "request.answer",
            json!({"id": "q1", "result": {"choice": "yes"}}),
        );
        assert_call(
            &clarify_lock_call("q1", "qu1", Some(json!("yes")), "swe"),
            "clarify.lock",
            json!({"request_id": "q1", "question_id": "qu1", "answer": "yes", "profile": "swe"}),
        );
        assert_call(
            &clarify_lock_call("q1", "qu1", None, ""),
            "clarify.lock",
            json!({"request_id": "q1", "question_id": "qu1"}),
        );
        assert_call(
            &prompt_background_call("s1", "", "run this"),
            "prompt.background",
            json!({"session_id": "s1", "text": "run this"}),
        );
        assert_call(
            &prompt_btw_call("s1", "", "quick q"),
            "prompt.btw",
            json!({"session_id": "s1", "text": "quick q"}),
        );
    }

    #[test]
    fn rpc_input_builders() {
        assert_call(
            &complete_path_call(Some("@src"), Some("/tmp"), Some("s1"), ""),
            "complete.path",
            json!({"word": "@src", "cwd": "/tmp", "session_id": "s1"}),
        );
        assert_call(
            &complete_slash_call(Some("/he"), None, "swe"),
            "complete.slash",
            json!({"text": "/he", "profile": "swe"}),
        );
        assert_call(
            &input_detect_drop_call("s1", Some("@/tmp/x.png"), ""),
            "input.detect_drop",
            json!({"session_id": "s1", "text": "@/tmp/x.png"}),
        );
        assert_call(
            &paste_collapse_call(Some("a\nb")),
            "paste.collapse",
            json!({"text": "a\nb"}),
        );
        assert_call(
            &commands_catalog_call(Some("s1"), ""),
            "commands.catalog",
            json!({"session_id": "s1"}),
        );
        assert_call(
            &command_resolve_call(Some("help"), ""),
            "command.resolve",
            json!({"name": "help"}),
        );
        assert_call(
            &message_react_call("s1", "", Some(7), None, Some("👍"), Some("user")),
            "message.react",
            json!({"session_id": "s1", "row_id": 7, "emoji": "👍", "author": "user"}),
        );
        assert_call(
            &message_react_call("s1", "", Some(7), None, None, None),
            "message.react",
            json!({"session_id": "s1", "row_id": 7, "emoji": null}),
        );
    }

    #[test]
    fn decodes_rest_payloads() {
        let page: PaginatedSessions = decode(json!({
            "sessions": [{
                "id": "s1", "title": "Demo", "source": "desktop", "model": "hermes-4",
                "started_at": 1700000000.5, "ended_at": null, "last_active": 1700000100.0,
                "message_count": 2, "tool_call_count": 0, "input_tokens": 10, "output_tokens": 5,
                "preview": "hi", "parent_session_id": null, "archived": 0, "pinned": 1,
                "hidden": 0, "unread": false, "cwd": "/tmp", "profile": "default",
                "is_default_profile": true, "is_active": true, "extra": "ignored"
            }],
            "total": 1, "limit": 20, "offset": 0,
            "profile_totals": {"default": 1},
            "errors": [{"profile": "swe", "error": "locked"}],
            "storage": {"swe": "corrupt"}
        }))
        .unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.sessions[0].title.as_deref(), Some("Demo"));
        assert!(!page.sessions[0].archived);
        assert!(page.sessions[0].pinned);
        assert_eq!(page.sessions[0].hidden, Some(false));
        assert_eq!(page.profile_totals["default"], 1);
        assert_eq!(page.errors[0].profile, "swe");
        assert_eq!(page.storage["swe"], "corrupt");
        assert_eq!(
            decode::<PaginatedSessions>(json!({})).unwrap(),
            PaginatedSessions::default()
        );

        let hits: SessionSearchResponse = decode(json!({
            "results": [{
                "session_id": "s1", "lineage_root": "s1", "id": "s1", "title": null,
                "source": "desktop", "model": null, "started_at": 1.0, "last_active": null,
                "is_active": false, "message_count": 2, "snippet": "hi", "role": "user"
            }]
        }))
        .unwrap();
        assert_eq!(hits.results[0].snippet, "hi");
        assert_eq!(hits.results[0].title, None);
        assert_eq!(hits.results[0].last_active, None);

        let detail: SessionDetail = decode(json!({
            "id": "s1", "title": "Demo", "started_at": 1.0, "last_active": 2.0,
            "archived": 0, "pinned": 0, "hidden": null, "runtime_revision": 7,
            "profile": "default", "is_default_profile": true
        }))
        .unwrap();
        assert_eq!(detail.id, "s1");
        assert_eq!(detail.runtime_revision, 7);
        assert!(!detail.archived);
        assert_eq!(detail.hidden, None);
        assert_eq!(decode::<SessionDetail>(json!({})).unwrap(), SessionDetail::default());

        let msgs: SessionMessagesPage = decode(json!({
            "session_id": "s1", "profile": "default",
            "messages": [
                {"role": "user", "content": "hi", "text": "hi", "row_id": 3, "timestamp": 1.5},
                {"role": "assistant", "content": [{"type": "text", "text": "hello"}], "row_id": 4}
            ],
            "pagination": {"limit": 100, "offset": 0, "order": "oldest", "returned": 2}
        }))
        .unwrap();
        assert_eq!(msgs.messages.len(), 2);
        assert_eq!(msgs.messages[0].row_id, Some(3));
        assert!(msgs.messages[1].content.is_array());
        assert_eq!(msgs.pagination.order.as_deref(), Some("oldest"));

        let around: MessagesAroundPage = decode(json!({
            "session_id": "s1", "profile": "default",
            "messages": [{"role": "user", "text": "hi", "row_id": 3}],
            "pagination": {"row_id": 3, "limit": 120, "returned": 1, "order": "oldest",
                           "offset": 4, "total": 9, "has_older": true, "has_newer": true}
        }))
        .unwrap();
        assert_eq!(around.pagination.row_id, 3);
        assert!(around.pagination.has_older);

        let side: SidebarPayload = decode(json!({
            "recents": {
                "sessions": [{"id": "s1", "started_at": 1.0, "last_active": 2.0}],
                "profiles_truncated": {"default": true},
                "profiles_usage": {"default": {"total": 5}},
            },
            "cron": {"sessions": []},
            "messaging": {"sessions": [], "total": 0},
            "errors": [], "storage": {}
        }))
        .unwrap();
        assert_eq!(side.recents.sessions.len(), 1);
        assert!(side.recents.profiles_truncated["default"]);
        assert_eq!(decode::<SidebarPayload>(json!({})).unwrap(), SidebarPayload::default());
    }

    #[test]
    fn decodes_rpc_payloads() {
        let act: ActivateResult = decode(json!({
            "session_id": "s1", "message_count": 2, "stored_session_id": "s1",
            "resumed": "s1", "session_key": "k1", "messages_omitted": true,
            "running": false,
            "messages": [{"role": "user", "content": "hi", "row_id": 1}],
            "info": {"model": "hermes-4", "provider": "nous", "cwd": "/tmp",
                     "project": {"id": "p1", "slug": "p", "name": "P"},
                     "usage": {"total": 5, "calls": 1}},
            "pending_approval": null
        }))
        .unwrap();
        assert_eq!(act.session_id, "s1");
        assert_eq!(act.info.project.as_ref().unwrap().slug, "p");
        assert_eq!(act.info.usage.as_ref().unwrap().total, 5);
        assert_eq!(act.messages.len(), 1);
        assert_eq!(decode::<ActivateResult>(json!({})).unwrap(), ActivateResult::default());

        let list: ActiveListResult = decode(json!({
            "sessions": [{"current": true, "id": "s1", "last_active": 2.0, "message_count": 1,
                          "model": "m", "preview": "hi", "session_key": "k1", "started_at": 1.0,
                          "status": "idle", "title": "T"}]
        }))
        .unwrap();
        assert!(list.sessions[0].current);

        let h: History = decode(json!({
            "count": 1, "messages": [{"role": "assistant", "content": "hello", "row_id": 2}]
        }))
        .unwrap();
        assert_eq!(h.count, 1);

        let ctx: ContextBreakdown = decode(json!({
            "categories": [{"color": "#f00", "id": "system", "label": "System", "tokens": 12}],
            "context_max": 100000, "context_percent": 3, "context_used": 3000,
            "estimated_total": 3200, "context_estimated": true, "context_source": "estimate",
            "model": "hermes-4",
            "context_files": [{"label": "AGENTS.md", "path": "/tmp/AGENTS.md", "chars": 40,
                               "est_tokens": 10, "loaded": true, "status": "ok"}]
        }))
        .unwrap();
        assert_eq!(ctx.categories[0].tokens, 12);
        assert!(ctx.context_files[0].loaded);

        let ctl: ControlResult = decode(json!({
            "control": {
                "goal": null,
                "loop": null,
                "heartbeat": {"prompt": "check", "status": "active", "interval_seconds": 60,
                              "created_at": 1.0, "last_fired_at": 2.0, "fire_count": 3},
                "revision": "7", "updated_at": 3.0
            },
            "dispatch": {"type": "notice", "notice": "No heartbeat set."}
        }))
        .unwrap();
        assert_eq!(ctl.control.heartbeat.as_ref().unwrap().fire_count, 3);
        assert_eq!(ctl.dispatch.kind.as_deref(), Some("notice"));

        let ev: EventsSince = decode(json!({
            "events": [{"type": "message.start", "seq": 1}], "latest_seq": 1,
            "truncated": false, "count": 1, "epoch": "e1",
            "open_requests": [{"id": "r1"}]
        }))
        .unwrap();
        assert_eq!(ev.events.len(), 1);
        assert_eq!(ev.open_requests.len(), 1);

        let foreign: ForeignList = decode(json!({
            "sessions": [{"id": "f1", "source": "codex", "label": "Codex", "title": "T",
                          "cwd": null, "mtime": 1.0, "turn_count": 3, "excerpt": "hi"}],
            "next_offset": null, "host": "Alpha", "unreadable": 0
        }))
        .unwrap();
        assert_eq!(foreign.host, "Alpha");
        assert_eq!(foreign.sessions[0].turn_count, 3);

        let br: BranchStored = decode(json!({
            "session_id": "n1", "stored_session_id": "n1", "message_count": 2,
            "messages_omitted": false, "info": {"model": "m", "cwd": "/tmp"}
        }))
        .unwrap();
        assert_eq!(br.info.cwd, "/tmp");

        let usage: Usage = decode(json!({
            "model": "hermes-4", "input": 10, "output": 5, "total": 15, "calls": 1,
            "cost_usd": 0.01, "cache_hit_pct": 50
        }))
        .unwrap();
        assert_eq!(usage.total, 15);
        assert_eq!(usage.cache_hit_pct, Some(50));

        let items: SlashCompletions = decode(json!({
            "items": [{"text": "/help", "display": "help", "meta": "Show help"}],
            "replace_from": 0
        }))
        .unwrap();
        assert_eq!(items.items[0].text, "/help");

        let cat: CommandCatalog = decode(json!({
            "pairs": [["help", "Show help"]],
            "categories": [{"name": "Session", "pairs": [["new", "New chat"]]}],
            "skill_count": 2, "warning": ""
        }))
        .unwrap();
        assert_eq!(cat.pairs[0][0], "help");
        assert_eq!(cat.categories[0].name, "Session");

        let react: Reactions = decode(json!({
            "row_id": 7,
            "reactions": [{"emoji": "👍", "author": "user", "at": 1.0, "seen": false}]
        }))
        .unwrap();
        assert_eq!(react.reactions[0].author, "user");

        let close: SessionClosed = decode(json!({"closed": true, "messages": ["turn stopped"]})).unwrap();
        assert!(close.closed);

        let pr = preview_restart_call("s1", "http://localhost:3000", "/w", "TypeError: x", "");
        assert_eq!(pr.method, "preview.restart");
        assert_eq!(
            pr.params,
            json!({"session_id": "s1", "url": "http://localhost:3000", "cwd": "/w",
                   "context": "TypeError: x"})
        );
        assert_eq!(
            preview_restart_call("s1", "http://localhost:3000", "", "", "swe").params,
            json!({"session_id": "s1", "url": "http://localhost:3000", "profile": "swe"})
        );

        assert_eq!(
            llm_oneshot_call("", "", "Summarize.", "text", &LlmOneshotOptions::default(), "").params,
            json!({ "instructions": "Summarize.", "input": "text" })
        );
        assert_eq!(
            llm_oneshot_call(
                "s1",
                "tpl",
                "",
                "",
                &LlmOneshotOptions {
                    variables: Some(&json!({"n": "1"})),
                    task: "title_generation",
                    max_tokens: Some(64),
                    // Exact in f32 -> f64 (0.2 would round-trip as 0.20000000298023224).
                    temperature: Some(0.25),
                },
                "swe"
            )
            .params,
            json!({"session_id": "s1", "template": "tpl", "variables": {"n": "1"},
                   "task": "title_generation", "max_tokens": 64, "temperature": 0.25,
                   "profile": "swe"})
        );
        let one: LlmOneshot = decode(json!({"text": "Hello"})).unwrap();
        assert_eq!(one.text, "Hello");
        assert_eq!(decode::<LlmOneshot>(json!({})).unwrap(), LlmOneshot::default());

        assert_eq!(
            image_generate_call("a cat", "square", false, None).params,
            json!({ "prompt": "a cat", "aspect_ratio": "square" })
        );
        assert_eq!(
            image_generate_call("", "", true, Some(1000)).params,
            json!({ "probe": true, "max_bytes": 1000 })
        );
        let ig: ImageGenerated = decode(json!({
            "available": true, "success": true, "image": "/tmp/a.png"
        }))
        .unwrap();
        assert!(ig.success && ig.image_data.is_none());
        assert_eq!(decode::<ImageGenerated>(json!({})).unwrap(), ImageGenerated::default());
    }
}
