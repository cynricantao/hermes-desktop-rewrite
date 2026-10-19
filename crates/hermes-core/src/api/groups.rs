//! Group chats and the bot relay: `groups.*` and `bot_relay.*` RPCs plus
//! `bot_relay.outbox.pending`.
//!
//! Handlers: `tui_gateway/methods_groups.py`, `tui_gateway/methods_bot_relay.py` (room
//! shapes from `gateway/hosted_rooms.py` / `gateway/hosted_room_replicas.py`, RoomLink
//! catalog from `gateway/hosted_room_peer.py`). Contracts:
//! `tui_gateway/contracts/groups_bot_relay.py`.
//!
//! # Safety classes (mirrored in every wrapper's doc comment)
//! * **Local reads**: [`groups_capabilities`], [`groups_list`], [`groups_state`],
//!   [`groups_log`], [`groups_replica_state`]. Safe to call on a live backend.
//! * **Spawns agent work / spends model calls**: [`groups_create`] (starts the room's driver),
//!   [`groups_send`] (appends `message.user` and wakes the driver → member turns),
//!   [`groups_approve`] and [`groups_retry`] (resume a member's run), [`groups_rename`] and
//!   [`groups_stop`] (drive the room's runtime). Builder-only in the live test.
//! * **Destructive**: [`groups_disband`] tombstones the room id permanently.
//! * **Network**: [`groups_peer_invite`] mints a grant for another installation,
//!   [`groups_peer_register`] probes a peer gateway over HTTP, [`groups_replicate`] /
//!   [`groups_promote`] / [`groups_demote`] move room authority between gateways.
//! * **External side effects**: [`bot_relay_deliver`] runs a whole turn on another profile's
//!   Bot Chat and talks to that profile's gateway; [`bot_relay_outbox_drain`] *claims* queued
//!   envelopes (they leave the outbox for the caller); [`bot_relay_reply`] resolves another
//!   connection's waiter; [`bot_relay_roster_sync`] replaces this install's view of remote
//!   agents.
//!
//! # `profile`
//! Every `groups.*` method is profile-scoped: the builders always send `profile` (`""` = the
//! gateway's own profile, which is what the handler falls back to). The relay methods are
//! **install-wide** — their handlers only read the fields listed on each builder —
//! except [`bot_relay_deliver`], where `profile` is the *target* profile on this gateway.

use std::collections::BTreeMap;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::api::{ApiError, Call, call_of, decode};
use crate::rpc::Client;

// ── Timeouts ────────────────────────────────────────────────────────────────────

/// `bot_relay.deliver` blocks a full relayed turn on the target gateway: lock wait (120 s) +
/// two 600 s attempts + 180 s settlement, mirroring
/// `apps/desktop/src/plugins/hermes-bots/relay-budget.ts` (`RELAY_DELIVER_TIMEOUT_MS`). The
/// client's 30 s default would kill every long turn.
pub const RELAY_DELIVER_TIMEOUT: Duration = Duration::from_secs(1500);

async fn call_as_timeout<T: DeserializeOwned>(client: &Client, c: &Call, timeout: Duration) -> Result<T, ApiError> {
    let v = client.request_with_timeout(c.method, c.params.clone(), timeout).await?;
    decode(v)
}

// ── Shared room shapes ──────────────────────────────────────────────────────────

/// One roster row (`hosted_room_discussion.validate_roster`); legacy rooms may carry
/// pre-normalisation rows, so unknown fields are ignored.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct RoomMember {
    pub member_id: String,
    pub profile: String,
    pub handle: String,
    pub display_name: Option<String>,
    pub target: Value,
}

/// A roster row as the client proposes it (`groups.create`, `groups.replicate`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RoomMemberInput {
    pub member_id: String,
    pub profile: String,
    pub handle: String,
    pub display_name: Option<String>,
    /// `{"kind": "local", "profile": …}` today; free-form by contract.
    pub target: Option<Value>,
}

impl RoomMemberInput {
    pub fn new(member_id: impl Into<String>, profile: impl Into<String>, handle: impl Into<String>) -> Self {
        Self {
            member_id: member_id.into(),
            profile: profile.into(),
            handle: handle.into(),
            display_name: None,
            target: None,
        }
    }

    fn to_json(&self) -> Value {
        let mut row = json!({
            "member_id": self.member_id,
            "profile": self.profile,
            "handle": self.handle,
        });
        if let Some(name) = &self.display_name {
            row["display_name"] = json!(name);
        }
        if let Some(target) = &self.target {
            row["target"] = target.clone();
        }
        row
    }
}

fn members_json(members: &[RoomMemberInput]) -> Value {
    Value::Array(members.iter().map(RoomMemberInput::to_json).collect())
}

/// `{"kind", "id"}` — the actor a room event is attributed to (server-owned).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct RoomActor {
    pub kind: String,
    pub id: String,
}

/// `gateway/hosted_rooms.py::_event_from_row`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct RoomEvent {
    pub room_id: String,
    pub seq: i64,
    pub event_id: String,
    pub kind: String,
    pub actor: RoomActor,
    pub authority_epoch: Option<i64>,
    pub payload: Value,
    pub created_at: f64,
    pub idempotent: bool,
}

/// `gateway/hosted_rooms.py::_room_from_row` plus the branch-only keys `adopted` / `claim_event`
/// (legacy adoption), `authority_claim` (`groups.state`) and `event` (`groups.rename`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct Room {
    pub room_id: String,
    pub name: String,
    pub members: Vec<RoomMember>,
    pub authority_gateway_id: String,
    pub authority_epoch: i64,
    pub revision: i64,
    pub created_at: f64,
    pub updated_at: f64,
    pub idempotent: bool,
    pub disbanded_at: Option<f64>,
    pub latest_seq: Option<i64>,
    pub adopted: Option<bool>,
    pub claim_event: Option<RoomEvent>,
    pub authority_claim: Option<RoomEvent>,
    pub event: Option<RoomEvent>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct RoomAuthority {
    pub gateway_id: String,
    pub epoch: i64,
}

// ── RoomLink catalog ───────────────────────────────────────────────────────────

/// `gateway/hosted_room_execution_policy.py::execution_policy_mapping`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct RoomExecutionPolicy {
    pub version: i64,
    pub target_profile: String,
    pub enabled_toolsets: Vec<String>,
    pub approval_mode: String,
    pub max_iterations: i64,
    pub policy_digest: String,
}

/// `GatewayRoomCatalog.endpoint_mapping`: `url`/`transport_security` when reachable,
/// `reason` when not.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct RoomLinkEndpoint {
    pub available: bool,
    pub url: Option<String>,
    pub transport_security: Option<String>,
    pub reason: Option<String>,
}

/// `gateway/hosted_room_peer.py::GatewayRoomCatalog.as_mapping`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct RoomLinkCatalog {
    pub installation_id: String,
    pub protocol_versions: Vec<i64>,
    pub link_modes: Vec<String>,
    pub persistent_process: bool,
    pub text: bool,
    pub attachments: bool,
    pub execution_policy: RoomExecutionPolicy,
    pub catalog_digest: String,
    pub endpoint: Option<RoomLinkEndpoint>,
}

/// `enabled` with `profile`/`catalog`/`endpoint`, or disabled with a `reason`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct RoomLinkStatus {
    pub enabled: bool,
    pub profile: Option<String>,
    pub catalog: Option<RoomLinkCatalog>,
    pub endpoint: Option<RoomLinkEndpoint>,
    pub reason: Option<String>,
}

// ── groups.capabilities ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct GroupsCapabilities {
    pub protocol_version: i64,
    pub driver: bool,
    pub persistent_process: bool,
    pub authority_gateway_id: String,
    pub room_link: RoomLinkStatus,
    pub features: Vec<String>,
    pub methods: Vec<String>,
    pub max_log_limit: i64,
}

/// `groups.capabilities` — describe the hosted-room protocol this gateway implements.
pub fn groups_capabilities_call(profile: &str) -> Call {
    call_of("groups.capabilities", json!({ "profile": profile }))
}

/// Negotiate the gateway's group-chat protocol. A local read — no agent work.
pub async fn groups_capabilities(client: &Client, profile: &str) -> Result<GroupsCapabilities, ApiError> {
    crate::api::call_as(client, &groups_capabilities_call(profile)).await
}

// ── groups.list / create / state ───────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct GroupsList {
    pub rooms: Vec<Room>,
    pub next_offset: Option<i64>,
}

/// `groups.list` — rooms hosted by this gateway, most recently changed first.
pub fn groups_list_call(profile: &str, limit: i64, offset: i64, include_disbanded: bool) -> Call {
    call_of(
        "groups.list",
        json!({
            "profile": profile, "limit": limit, "offset": offset,
            "include_disbanded": include_disbanded,
        }),
    )
}

/// Page this gateway's hosted rooms. A local read.
///
/// Paging: `next_offset` is `None` on the last page; the backend computes it as
/// `offset + limit` only when the page came back full.
pub async fn groups_list(
    client: &Client,
    profile: &str,
    limit: i64,
    offset: i64,
    include_disbanded: bool,
) -> Result<GroupsList, ApiError> {
    crate::api::call_as(client, &groups_list_call(profile, limit, offset, include_disbanded)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct GroupsCreate {
    pub room: Room,
}

/// `groups.create` — create one hosted room idempotently; authority is this gateway's stable
/// install identity (a client-supplied authority is ignored).
pub fn groups_create_call(profile: &str, room_id: &str, name: &str, members: &[RoomMemberInput]) -> Call {
    call_of(
        "groups.create",
        json!({
            "profile": profile, "room_id": room_id, "name": name,
            "members": members_json(members),
        }),
    )
}

/// Create a Group Chat. **Spawns a room driver** (it wakes the hosted-room runtime), so a
/// send by any member can spend model calls; the call itself only writes the room row.
/// Requires the gateway's room service to be running (`4123` otherwise).
pub async fn groups_create(
    client: &Client,
    profile: &str,
    room_id: &str,
    name: &str,
    members: &[RoomMemberInput],
) -> Result<GroupsCreate, ApiError> {
    crate::api::call_as(client, &groups_create_call(profile, room_id, name, members)).await
}

/// One live peer route on the room home.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct PeerRouteStatus {
    pub room_id: String,
    pub member_id: String,
    pub status: String,
}

/// `HostedRoomService.status(room_id)`; `pending_actions` rows are `{kind: retry, task_id}` or
/// the driver's approval action (`kind: approval` + run/session/approval context).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct RoomDriverStatus {
    pub running: bool,
    pub working: bool,
    pub blocked: bool,
    pub counts: BTreeMap<String, i64>,
    pub pending_actions: Vec<Value>,
    pub peer_routes: Vec<PeerRouteStatus>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct GroupsState {
    pub room: Room,
    pub driver_status: Option<RoomDriverStatus>,
}

/// `groups.state` — one room's replay cursor and fenced authority state.
pub fn groups_state_call(profile: &str, room_id: &str, include_disbanded: bool) -> Call {
    call_of(
        "groups.state",
        json!({
            "profile": profile, "room_id": room_id, "include_disbanded": include_disbanded,
        }),
    )
}

/// Read one room's state plus its live driver status. A local read (`driver_status` is
/// omitted for a disbanded room or when the room service is not running).
pub async fn groups_state(
    client: &Client,
    profile: &str,
    room_id: &str,
    include_disbanded: bool,
) -> Result<GroupsState, ApiError> {
    crate::api::call_as(client, &groups_state_call(profile, room_id, include_disbanded)).await
}

// ── groups.send / rename / log ─────────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct GroupsSend {
    pub event: RoomEvent,
    pub client_event_id: Option<String>,
    pub accepted: bool,
    pub driver_started: bool,
}

/// `groups.send` — append one inert `message.user` event idempotently (the actor is
/// server-owned). An omitted `event_id` lets the backend mint one.
pub fn groups_send_call(profile: &str, room_id: &str, event_id: Option<&str>, payload: Value) -> Call {
    let mut params = json!({
        "profile": profile, "room_id": room_id, "payload": payload,
    });
    if let Some(id) = event_id {
        params["event_id"] = json!(id);
    }
    call_of("groups.send", params)
}

/// Post a user message into a Group Chat. **Spends model calls**: the event wakes the room
/// driver and the addressed member(s) answer in their own profile. `payload` is validated
/// server-side (`validate_user_payload`: text + optional attachments) and is passed through
/// verbatim.
pub async fn groups_send(
    client: &Client,
    profile: &str,
    room_id: &str,
    event_id: Option<&str>,
    payload: Value,
) -> Result<GroupsSend, ApiError> {
    crate::api::call_as(client, &groups_send_call(profile, room_id, event_id, payload)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct GroupsRename {
    pub room: Room,
}

/// `groups.rename` — rename one room atomically with its replay event (`event_id` is required
/// and makes the rename idempotent).
pub fn groups_rename_call(profile: &str, room_id: &str, event_id: &str, name: &str) -> Call {
    call_of(
        "groups.rename",
        json!({
            "profile": profile, "room_id": room_id, "event_id": event_id, "name": name,
        }),
    )
}

/// Rename a Group Chat. **Wakes the room runtime** (a typed `room.renamed` event enters the
/// replay log the driver reads) but starts no turn by itself.
pub async fn groups_rename(
    client: &Client,
    profile: &str,
    room_id: &str,
    event_id: &str,
    name: &str,
) -> Result<GroupsRename, ApiError> {
    crate::api::call_as(client, &groups_rename_call(profile, room_id, event_id, name)).await
}

/// `gateway/hosted_rooms.py::read_events` page — also the `page` `groups.replicate` ingests.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct GroupsLog {
    pub events: Vec<RoomEvent>,
    pub cursor: i64,
    pub latest_seq: i64,
    pub has_more: bool,
    pub authority: RoomAuthority,
}

/// `groups.log` — monotonic room-log delta after `since_seq`, bounded by count and page bytes.
pub fn groups_log_call(profile: &str, room_id: &str, since_seq: i64, limit: i64, include_disbanded: bool) -> Call {
    call_of(
        "groups.log",
        json!({
            "profile": profile, "room_id": room_id, "since_seq": since_seq,
            "limit": limit, "include_disbanded": include_disbanded,
        }),
    )
}

/// Read a room's replay log delta. A local read. `cursor` is the last returned seq (or
/// `since_seq` when empty) and `has_more` means another page exists.
pub async fn groups_log(
    client: &Client,
    profile: &str,
    room_id: &str,
    since_seq: i64,
    limit: i64,
    include_disbanded: bool,
) -> Result<GroupsLog, ApiError> {
    crate::api::call_as(
        client,
        &groups_log_call(profile, room_id, since_seq, limit, include_disbanded),
    )
    .await
}

// ── groups.disband / stop / approve / retry ────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct RoomTombstone {
    pub room_id: String,
    pub disbanded_at: f64,
    pub idempotent: bool,
    pub history_expired: Option<bool>,
    pub event: Option<RoomEvent>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct GroupsDisband {
    pub tombstone: RoomTombstone,
}

/// `groups.disband` — permanently tombstone a room id after stopping its work and revoking
/// peer routes.
pub fn groups_disband_call(profile: &str, room_id: &str, cancel_id: Option<&str>) -> Call {
    let mut params = json!({ "profile": profile, "room_id": room_id });
    if let Some(id) = cancel_id {
        params["cancel_id"] = json!(id);
    }
    call_of("groups.disband", params)
}

/// Disband a Group Chat. **Destructive and irreversible**: the room id is tombstoned forever
/// (re-creating it fails) and the room's running work is cancelled.
pub async fn groups_disband(
    client: &Client,
    profile: &str,
    room_id: &str,
    cancel_id: Option<&str>,
) -> Result<GroupsDisband, ApiError> {
    crate::api::call_as(client, &groups_disband_call(profile, room_id, cancel_id)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct GroupsStop {
    pub cancelled: i64,
}

/// `groups.stop` — durably cancel queued or running work for one room.
pub fn groups_stop_call(profile: &str, room_id: &str, cancel_id: Option<&str>) -> Call {
    let mut params = json!({ "profile": profile, "room_id": room_id });
    if let Some(id) = cancel_id {
        params["cancel_id"] = json!(id);
    }
    call_of("groups.stop", params)
}

/// Cancel a room's queued/running member work. **Aborts in-flight turns** (each cancelled run
/// settles as cancelled for its profile); `cancelled` is the number of tasks stopped.
pub async fn groups_stop(
    client: &Client,
    profile: &str,
    room_id: &str,
    cancel_id: Option<&str>,
) -> Result<GroupsStop, ApiError> {
    crate::api::call_as(client, &groups_stop_call(profile, room_id, cancel_id)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct GroupsApprove {
    /// The local `approval.respond` answer, or a peer's run-action receipt.
    pub result: Value,
    pub approved: bool,
}

/// One exact approval to resolve (see [`groups_approve_call`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Approval {
    pub member_id: String,
    pub task_id: String,
    /// Fences a stale approval: the member's task generation the UI saw.
    pub execution_generation: i64,
    /// `server_requests.ApprovalChoice` — `once` / `session` / `always` / `deny`.
    pub choice: String,
    pub request_id: String,
}

/// `groups.approve` — resolve one exact approval raised by a local or peer room member.
pub fn groups_approve_call(profile: &str, room_id: &str, approval: &Approval) -> Call {
    call_of(
        "groups.approve",
        json!({
            "profile": profile, "room_id": room_id, "member_id": approval.member_id,
            "task_id": approval.task_id, "execution_generation": approval.execution_generation,
            "choice": approval.choice, "request_id": approval.request_id,
        }),
    )
}

/// Resolve a member's pending approval. **Resumes agent work** — the approved run continues
/// (and spends model calls); a peer member's approval round-trips to its gateway.
pub async fn groups_approve(
    client: &Client,
    profile: &str,
    room_id: &str,
    approval: &Approval,
) -> Result<GroupsApprove, ApiError> {
    crate::api::call_as(client, &groups_approve_call(profile, room_id, approval)).await
}

/// What `groups.retry` reports back about the re-queued task.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct RoomTaskReceipt {
    pub room_id: String,
    pub task_id: String,
    pub thread_id: String,
    pub turn_id: String,
    pub status: String,
    pub execution_generation: i64,
    pub cancel_generation: i64,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct GroupsRetry {
    pub task: RoomTaskReceipt,
    pub retried: bool,
}

/// `groups.retry` — re-run one indeterminate room task (only after the user confirms).
pub fn groups_retry_call(profile: &str, room_id: &str, task_id: &str) -> Call {
    call_of(
        "groups.retry",
        json!({ "profile": profile, "room_id": room_id, "task_id": task_id }),
    )
}

/// Retry an indeterminate member task. **Spends model calls**: the task is re-queued and run
/// again by its member's profile. Only call after an explicit user confirmation.
pub async fn groups_retry(
    client: &Client,
    profile: &str,
    room_id: &str,
    task_id: &str,
) -> Result<GroupsRetry, ApiError> {
    crate::api::call_as(client, &groups_retry_call(profile, room_id, task_id)).await
}

// ── replication / authority takeover ───────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct GroupsReplicate {
    pub room_id: String,
    pub stored_seq: i64,
    pub ingested: i64,
    pub authority: RoomAuthority,
    pub caught_up: bool,
}

/// `groups.replicate` — persist one authority-stamped replay page (a verbatim `groups.log`
/// result) into the local replica store; idempotent, refuses sequence gaps and epoch
/// regressions.
pub fn groups_replicate_call(
    profile: &str,
    room_id: &str,
    room_name: &str,
    members: &[RoomMemberInput],
    page: Value,
) -> Call {
    call_of(
        "groups.replicate",
        json!({
            "profile": profile, "room_id": room_id, "room_name": room_name,
            "members": members_json(members), "page": page,
        }),
    )
}

/// Store a replicated room page locally. **Cross-gateway replication**: the page must come
/// from the room's current authority; writing a forged page corrupts the local replica.
pub async fn groups_replicate(
    client: &Client,
    profile: &str,
    room_id: &str,
    room_name: &str,
    members: &[RoomMemberInput],
    page: Value,
) -> Result<GroupsReplicate, ApiError> {
    crate::api::call_as(
        client,
        &groups_replicate_call(profile, room_id, room_name, members, page),
    )
    .await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct GroupsReplicaState {
    pub room_id: String,
    pub name: String,
    pub members: Vec<RoomMember>,
    pub authority: RoomAuthority,
    pub last_seq: i64,
    pub latest_seq: i64,
    pub event_bytes: i64,
    pub created_at: f64,
    pub updated_at: f64,
}

/// `groups.replica_state` — the local replica's coverage and authority lineage.
pub fn groups_replica_state_call(profile: &str, room_id: &str) -> Call {
    call_of(
        "groups.replica_state",
        json!({ "profile": profile, "room_id": room_id }),
    )
}

/// Read one room replica's coverage. A local read (replica store, no network).
pub async fn groups_replica_state(
    client: &Client,
    profile: &str,
    room_id: &str,
) -> Result<GroupsReplicaState, ApiError> {
    crate::api::call_as(client, &groups_replica_state_call(profile, room_id)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct GroupsPromote {
    pub room_id: String,
    pub authority_gateway_id: String,
    pub authority_epoch: i64,
    pub previous_gateway_id: String,
    pub previous_epoch: i64,
    pub claim_seq: i64,
    pub latest_seq: i64,
}

/// `groups.promote` — continue a replicated room on THIS gateway at `epoch + 1`. The builder
/// always sets `confirm: true`; the handler refuses anything else.
pub fn groups_promote_call(profile: &str, room_id: &str, reason: &str) -> Call {
    call_of(
        "groups.promote",
        json!({
            "profile": profile, "room_id": room_id, "confirm": true, "reason": reason,
        }),
    )
}

/// Take over a replicated room's authority. **Dangerous split-brain operation**: `confirm`
/// asserts the previous authority can no longer commit. Two live authorities for one room
/// corrupt its log.
pub async fn groups_promote(
    client: &Client,
    profile: &str,
    room_id: &str,
    reason: &str,
) -> Result<GroupsPromote, ApiError> {
    crate::api::call_as(client, &groups_promote_call(profile, room_id, reason)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct GroupsDemote {
    pub room_id: String,
    pub authority_gateway_id: String,
    pub authority_epoch: i64,
    pub idempotent: bool,
}

/// `groups.demote` — fence this gateway's stale room authority against a proven newer epoch.
pub fn groups_demote_call(profile: &str, room_id: &str, observed_gateway_id: &str, observed_epoch: i64) -> Call {
    call_of(
        "groups.demote",
        json!({
            "profile": profile, "room_id": room_id,
            "observed_gateway_id": observed_gateway_id, "observed_epoch": observed_epoch,
        }),
    )
}

/// Fence this gateway's room authority after another gateway proved a newer epoch. **Makes
/// this gateway read-only for the room**; only correct when the peer really holds authority.
pub async fn groups_demote(
    client: &Client,
    profile: &str,
    room_id: &str,
    observed_gateway_id: &str,
    observed_epoch: i64,
) -> Result<GroupsDemote, ApiError> {
    crate::api::call_as(
        client,
        &groups_demote_call(profile, room_id, observed_gateway_id, observed_epoch),
    )
    .await
}

// ── peer routes (RoomLink) ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct GroupsPeerInvite {
    pub grant: String,
    pub target_profile: String,
    pub catalog: RoomLinkCatalog,
    pub endpoint: RoomLinkEndpoint,
}

/// Arguments of [`groups_peer_invite_call`]. Empty strings are sent as `""`, which the handler
/// treats like an absent value (`params.get(k) or ""`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PeerInvite {
    pub room_id: String,
    pub home_install_id: String,
    pub authority_gateway_id: String,
    pub authority_epoch: i64,
    pub member_id: String,
    /// Empty → the target mints one.
    pub grant_id: String,
    /// Absent → the backend default (3600 s); valid range 60 … 86400.
    pub ttl_seconds: Option<f64>,
}

/// `groups.peer.invite` — mint one target-issued room/profile grant for a prospective home.
pub fn groups_peer_invite_call(profile: &str, invite: &PeerInvite) -> Call {
    let mut params = json!({
        "profile": profile,
        "room_id": invite.room_id,
        "home_install_id": invite.home_install_id,
        "authority_gateway_id": invite.authority_gateway_id,
        "authority_epoch": invite.authority_epoch,
        "member_id": invite.member_id,
        "grant_id": invite.grant_id,
    });
    if let Some(ttl) = invite.ttl_seconds {
        params["ttl_seconds"] = json!(ttl);
    }
    call_of("groups.peer.invite", params)
}

/// Mint a RoomLink grant for another installation. **Network + security sensitive**: the grant
/// authorizes that installation's profile to run room turns as this member until it expires;
/// it is a bearer token — never log it.
pub async fn groups_peer_invite(
    client: &Client,
    profile: &str,
    invite: &PeerInvite,
) -> Result<GroupsPeerInvite, ApiError> {
    crate::api::call_as(client, &groups_peer_invite_call(profile, invite)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct GroupsPeerRevoke {
    pub revoked: bool,
}

/// `groups.peer.revoke` — revoke one target-issued grant using its exact profile scope.
pub fn groups_peer_revoke_call(profile: &str, grant: &str) -> Call {
    call_of("groups.peer.revoke", json!({ "profile": profile, "grant": grant }))
}

/// Revoke a peer grant. Revokes only grants this gateway issued (`4122` on a scope mismatch).
/// Pass the grant verbatim; it is a bearer token — never log it.
pub async fn groups_peer_revoke(client: &Client, profile: &str, grant: &str) -> Result<GroupsPeerRevoke, ApiError> {
    crate::api::call_as(client, &groups_peer_revoke_call(profile, grant)).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct GroupsPeerRegister {
    pub registered: bool,
    pub mode: String,
    pub transport_security: String,
    pub target_install_id: String,
    pub target_profile: String,
}

/// Arguments of [`groups_peer_register_call`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PeerRegister {
    pub room_id: String,
    pub member_id: String,
    pub target_url: String,
    pub target_profile: String,
    pub grant: String,
    /// A RoomLink catalog mapping as the invite returned it; decoded exactly server-side.
    pub catalog: Value,
    /// Absent → `cancel-<room_id>`.
    pub cancellation_scope_id: Option<String>,
    /// Absent → the target mints one.
    pub trace_id: Option<String>,
}

/// `groups.peer.register` — register and probe one scoped target route on the room home.
pub fn groups_peer_register_call(profile: &str, route: &PeerRegister) -> Call {
    let mut params = json!({
        "profile": profile,
        "room_id": route.room_id,
        "member_id": route.member_id,
        "target_url": route.target_url,
        "target_profile": route.target_profile,
        "grant": route.grant,
        "catalog": route.catalog,
    });
    if let Some(id) = &route.cancellation_scope_id {
        params["cancellation_scope_id"] = json!(id);
    }
    if let Some(id) = &route.trace_id {
        params["trace_id"] = json!(id);
    }
    call_of("groups.peer.register", params)
}

/// Attach a peer route to a room. **Network**: the target URL is validated and probed over
/// HTTP with the grant; a mismatching catalog/scope fails (`5120`/`4121`).
pub async fn groups_peer_register(
    client: &Client,
    profile: &str,
    route: &PeerRegister,
) -> Result<GroupsPeerRegister, ApiError> {
    crate::api::call_as(client, &groups_peer_register_call(profile, route)).await
}

// ── bot relay ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct BotRelayRosterSync {
    pub count: i64,
}

/// `bot_relay.roster.sync` — replace this gateway's view of agents on OTHER connections.
/// Each row is `{profile, handle, connection_id, connection_label?, title?, description?,
/// online?}`; invalid rows are dropped server-side (hence the open rows).
pub fn bot_relay_roster_sync_call(agents: &[Value]) -> Call {
    call_of("bot_relay.roster.sync", json!({ "agents": agents }))
}

/// Push the cross-connection agent roster. **External side effects**: this replaces (not
/// merges) the gateway's remote roster, so a partial push makes peers unresolvable until the
/// next full sync. Install-wide; no `profile` param.
pub async fn bot_relay_roster_sync(client: &Client, agents: &[Value]) -> Result<BotRelayRosterSync, ApiError> {
    crate::api::call_as(client, &bot_relay_roster_sync_call(agents)).await
}

/// One queued cross-connection envelope (`tools/bot_relay.py::enqueue_envelope`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct RelayEnvelope {
    pub id: String,
    pub created_at: f64,
    pub canonical_delivery_v1: Option<bool>,
    pub from_profile: String,
    pub from_handle: String,
    pub target_connection: String,
    pub target_profile: String,
    pub target_handle: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct BotRelayOutboxDrain {
    pub envelopes: Vec<RelayEnvelope>,
}

/// `bot_relay.outbox.drain` — claim every pending envelope queued on this gateway.
pub fn bot_relay_outbox_drain_call() -> Call {
    call_of("bot_relay.outbox.drain", json!({}))
}

/// Claim this gateway's pending relay envelopes. **Destructive to the mailbox**: claimed
/// envelopes move to `claimed/` atomically, so if the caller then fails to deliver them the
/// messages are not retried by anyone else.
pub async fn bot_relay_outbox_drain(client: &Client) -> Result<BotRelayOutboxDrain, ApiError> {
    crate::api::call_as(client, &bot_relay_outbox_drain_call()).await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct BotRelayDeliver {
    pub reply: String,
}

/// `bot_relay.deliver` — legacy transport bridge for one cross-connection DM; blocking.
pub fn bot_relay_deliver_call(
    target_profile: &str,
    message: &str,
    id: &str,
    from_profile: Option<&str>,
    from_handle: Option<&str>,
    from_connection: Option<&str>,
) -> Call {
    let mut params = json!({
        "profile": target_profile, "message": message, "id": id,
    });
    if let Some(v) = from_profile {
        params["from_profile"] = json!(v);
    }
    if let Some(v) = from_handle {
        params["from_handle"] = json!(v);
    }
    if let Some(v) = from_connection {
        params["from_connection"] = json!(v);
    }
    call_of("bot_relay.deliver", params)
}

/// Deliver a relayed DM into a Bot Chat on this gateway and return the one-turn reply.
/// **Spends model calls and blocks for a whole turn** (budgeted at [`RELAY_DELIVER_TIMEOUT`]);
/// `id` must be 32–64 lowercase hex (the delivery-id format, `4090` otherwise); an unknown
/// target profile answers `4092`. The sender fields are not trusted for an authenticated
/// client — the backend derives the author from the connection's identity.
pub async fn bot_relay_deliver(
    client: &Client,
    target_profile: &str,
    message: &str,
    id: &str,
    from_profile: Option<&str>,
    from_handle: Option<&str>,
    from_connection: Option<&str>,
) -> Result<BotRelayDeliver, ApiError> {
    call_as_timeout(
        client,
        &bot_relay_deliver_call(target_profile, message, id, from_profile, from_handle, from_connection),
        RELAY_DELIVER_TIMEOUT,
    )
    .await
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct BotRelayReply {
    pub ok: bool,
}

/// `bot_relay.reply` — write a relayed `reply` and/or `error` (+ optional typed `reason`) for
/// envelope `id` so the sender-side waiter resolves.
pub fn bot_relay_reply_call(id: &str, reply: &str, error: &str, reason: &str) -> Call {
    call_of(
        "bot_relay.reply",
        json!({ "id": id, "reply": reply, "error": error, "reason": reason }),
    )
}

/// Resolve another connection's envelope waiter. **External side effects**: the payload is
/// delivered to a waiting peer's Bot Chat; an empty `id` answers `4093`, an unknown/settled
/// envelope `4094`.
pub async fn bot_relay_reply(
    client: &Client,
    id: &str,
    reply: &str,
    error: &str,
    reason: &str,
) -> Result<BotRelayReply, ApiError> {
    crate::api::call_as(client, &bot_relay_reply_call(id, reply, error, reason)).await
}

// ── bot_relay.outbox.pending notification ──────────────────────────────────────

/// Event name of the relay outbox signal (`tui_gateway/contracts/events.py`).
pub const BOT_RELAY_OUTBOX_PENDING: &str = "bot_relay.outbox.pending";

/// Payload of [`BOT_RELAY_OUTBOX_PENDING`]. The backend sends `{}` today
/// (`ChangeSignalPayload`, `change_watcher._bot_relay_outbox_sig`); unknown keys are kept in
/// [`BotRelayOutboxPending::extra`] so a later field cannot break decoding.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct BotRelayOutboxPending {
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

/// Recognise the relay outbox signal. The payload carries no detail: the reaction is always
/// "drain the outbox" ([`bot_relay_outbox_drain`]).
pub fn parse_bot_relay_outbox_pending(event_type: &str, payload: &Value) -> Option<BotRelayOutboxPending> {
    if event_type != BOT_RELAY_OUTBOX_PENDING {
        return None;
    }
    Some(serde_json::from_value(payload.clone()).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member() -> RoomMemberInput {
        RoomMemberInput::new("alpha", "alpha", "@alpha")
    }

    fn room() -> Value {
        json!({
            "room_id": "room-1", "name": "Planning", "members": [
                {"member_id": "alpha", "profile": "alpha", "handle": "@alpha",
                 "display_name": "Alpha", "target": {"kind": "local", "profile": "alpha"}},
                {"member_id": "beta", "profile": "beta", "handle": "@beta", "target": {}}
            ],
            "authority_gateway_id": "gw-1", "authority_epoch": 3, "revision": 7,
            "created_at": 1700000000.5, "updated_at": 1700000010.25,
            "idempotent": false, "latest_seq": 12
        })
    }

    #[test]
    fn capabilities_builder() {
        let c = groups_capabilities_call("swe");
        assert_eq!(c.method, "groups.capabilities");
        assert_eq!(c.params, json!({"profile": "swe"}));
        assert_eq!(groups_capabilities_call("").params, json!({"profile": ""}));
    }

    #[test]
    fn list_builder() {
        let c = groups_list_call("", 100, 200, true);
        assert_eq!(c.method, "groups.list");
        assert_eq!(
            c.params,
            json!({"profile": "", "limit": 100, "offset": 200, "include_disbanded": true})
        );
    }

    #[test]
    fn create_builder_and_member_rows() {
        let mut beta = RoomMemberInput::new("beta", "beta", "@beta");
        beta.display_name = Some("Beta".into());
        beta.target = Some(json!({"kind": "local", "profile": "beta"}));
        let c = groups_create_call("swe", "room-9", "Planning", &[member(), beta]);
        assert_eq!(c.method, "groups.create");
        assert_eq!(
            c.params,
            json!({
                "profile": "swe", "room_id": "room-9", "name": "Planning",
                "members": [
                    {"member_id": "alpha", "profile": "alpha", "handle": "@alpha"},
                    {"member_id": "beta", "profile": "beta", "handle": "@beta",
                     "display_name": "Beta", "target": {"kind": "local", "profile": "beta"}}
                ]
            })
        );
    }

    #[test]
    fn state_and_log_builders() {
        let c = groups_state_call("swe", "room-1", false);
        assert_eq!(c.method, "groups.state");
        assert_eq!(
            c.params,
            json!({"profile": "swe", "room_id": "room-1", "include_disbanded": false})
        );
        let c = groups_log_call("swe", "room-1", 4, 50, true);
        assert_eq!(c.method, "groups.log");
        assert_eq!(
            c.params,
            json!({"profile": "swe", "room_id": "room-1", "since_seq": 4, "limit": 50,
                   "include_disbanded": true})
        );
    }

    #[test]
    fn send_builder_optional_event_id() {
        let payload = json!({"kind": "message.user", "text": "hi", "attachments": []});
        let c = groups_send_call("swe", "room-1", Some("ev-7"), payload.clone());
        assert_eq!(c.method, "groups.send");
        assert_eq!(
            c.params,
            json!({"profile": "swe", "room_id": "room-1", "event_id": "ev-7", "payload": payload})
        );
        let c = groups_send_call("", "room-1", None, json!({"text": "hi"}));
        assert_eq!(
            c.params,
            json!({"profile": "", "room_id": "room-1", "payload": {"text": "hi"}})
        );
    }

    #[test]
    fn rename_stop_disband_builders() {
        let c = groups_rename_call("swe", "room-1", "ev-8", "New name");
        assert_eq!(c.method, "groups.rename");
        assert_eq!(
            c.params,
            json!({"profile": "swe", "room_id": "room-1", "event_id": "ev-8", "name": "New name"})
        );

        let c = groups_stop_call("", "room-1", Some("cancel-3"));
        assert_eq!(c.method, "groups.stop");
        assert_eq!(
            c.params,
            json!({"profile": "", "room_id": "room-1", "cancel_id": "cancel-3"})
        );
        assert_eq!(
            groups_stop_call("", "room-1", None).params,
            json!({"profile": "", "room_id": "room-1"})
        );

        let c = groups_disband_call("", "room-1", None);
        assert_eq!(c.method, "groups.disband");
        assert_eq!(c.params, json!({"profile": "", "room_id": "room-1"}));
        assert_eq!(
            groups_disband_call("", "room-1", Some("room-disbanded")).params,
            json!({"profile": "", "room_id": "room-1", "cancel_id": "room-disbanded"})
        );
    }

    #[test]
    fn approve_and_retry_builders() {
        let approval = Approval {
            member_id: "beta".into(),
            task_id: "task-2".into(),
            execution_generation: 5,
            choice: "once".into(),
            request_id: "req-9".into(),
        };
        let c = groups_approve_call("swe", "room-1", &approval);
        assert_eq!(c.method, "groups.approve");
        assert_eq!(
            c.params,
            json!({"profile": "swe", "room_id": "room-1", "member_id": "beta", "task_id": "task-2",
                   "execution_generation": 5, "choice": "once", "request_id": "req-9"})
        );
        let c = groups_retry_call("", "room-1", "task-2");
        assert_eq!(c.method, "groups.retry");
        assert_eq!(
            c.params,
            json!({"profile": "", "room_id": "room-1", "task_id": "task-2"})
        );
    }

    #[test]
    fn replication_builders() {
        let page = json!({"events": [], "cursor": 0, "latest_seq": 0, "has_more": false,
                          "authority": {"gateway_id": "gw-1", "epoch": 2}});
        let c = groups_replicate_call("", "room-1", "Planning", &[member()], page.clone());
        assert_eq!(c.method, "groups.replicate");
        assert_eq!(
            c.params,
            json!({"profile": "", "room_id": "room-1", "room_name": "Planning",
                   "members": [{"member_id": "alpha", "profile": "alpha", "handle": "@alpha"}],
                   "page": page})
        );

        assert_eq!(
            groups_replica_state_call("", "room-1").params,
            json!({"profile": "", "room_id": "room-1"})
        );

        let c = groups_promote_call("", "room-1", "authority-unreachable");
        assert_eq!(c.method, "groups.promote");
        assert_eq!(
            c.params,
            json!({"profile": "", "room_id": "room-1", "confirm": true,
                   "reason": "authority-unreachable"})
        );

        let c = groups_demote_call("", "room-1", "gw-2", 4);
        assert_eq!(c.method, "groups.demote");
        assert_eq!(
            c.params,
            json!({"profile": "", "room_id": "room-1",
                   "observed_gateway_id": "gw-2", "observed_epoch": 4})
        );
    }

    #[test]
    fn peer_builders() {
        let invite = PeerInvite {
            room_id: "room-1".into(),
            home_install_id: "install-a".into(),
            member_id: "beta".into(),
            ..PeerInvite::default()
        };
        let c = groups_peer_invite_call("swe", &invite);
        assert_eq!(c.method, "groups.peer.invite");
        assert_eq!(
            c.params,
            json!({"profile": "swe", "room_id": "room-1", "home_install_id": "install-a",
                   "authority_gateway_id": "", "authority_epoch": 0, "member_id": "beta",
                   "grant_id": ""})
        );
        let invite = PeerInvite {
            ttl_seconds: Some(600.0),
            ..invite
        };
        assert_eq!(groups_peer_invite_call("", &invite).params["ttl_seconds"], json!(600.0));

        assert_eq!(
            groups_peer_revoke_call("", "grant-token").params,
            json!({"profile": "", "grant": "grant-token"})
        );

        let route = PeerRegister {
            room_id: "room-1".into(),
            member_id: "beta".into(),
            target_url: "https://peer.example".into(),
            target_profile: "beta".into(),
            grant: "grant-token".into(),
            catalog: json!({"installation_id": "install-b"}),
            ..PeerRegister::default()
        };
        let c = groups_peer_register_call("", &route);
        assert_eq!(c.method, "groups.peer.register");
        assert_eq!(
            c.params,
            json!({"profile": "", "room_id": "room-1", "member_id": "beta",
                   "target_url": "https://peer.example", "target_profile": "beta",
                   "grant": "grant-token", "catalog": {"installation_id": "install-b"}})
        );
        let route = PeerRegister {
            cancellation_scope_id: Some("cancel-1".into()),
            trace_id: Some("trace-1".into()),
            ..route
        };
        let params = groups_peer_register_call("", &route).params;
        assert_eq!(params["cancellation_scope_id"], json!("cancel-1"));
        assert_eq!(params["trace_id"], json!("trace-1"));
    }

    #[test]
    fn relay_builders() {
        let rows = vec![json!({"profile": "beta", "handle": "@beta", "connection_id": "c2"})];
        let c = bot_relay_roster_sync_call(&rows);
        assert_eq!(c.method, "bot_relay.roster.sync");
        assert_eq!(c.params, json!({"agents": rows}));

        let c = bot_relay_outbox_drain_call();
        assert_eq!(c.method, "bot_relay.outbox.drain");
        assert_eq!(c.params, json!({}));

        let c = bot_relay_deliver_call(
            "beta",
            "hello",
            "0123456789abcdef0123456789abcdef",
            Some("alpha"),
            Some("@alpha"),
            Some("conn-1"),
        );
        assert_eq!(c.method, "bot_relay.deliver");
        assert_eq!(
            c.params,
            json!({"profile": "beta", "message": "hello",
                   "id": "0123456789abcdef0123456789abcdef",
                   "from_profile": "alpha", "from_handle": "@alpha",
                   "from_connection": "conn-1"})
        );
        let c = bot_relay_deliver_call("beta", "hello", "0123456789abcdef0123456789abcdef", None, None, None);
        assert_eq!(
            c.params,
            json!({"profile": "beta", "message": "hello",
                   "id": "0123456789abcdef0123456789abcdef"})
        );

        let c = bot_relay_reply_call("env-1", "pong", "", "");
        assert_eq!(c.method, "bot_relay.reply");
        assert_eq!(
            c.params,
            json!({"id": "env-1", "reply": "pong", "error": "", "reason": ""})
        );
    }

    #[test]
    fn decodes_a_list_reply() {
        let page: GroupsList = decode(json!({
            "rooms": [room()],
            "next_offset": 100,
            "extra_key": true
        }))
        .unwrap();
        assert_eq!(page.rooms.len(), 1);
        assert_eq!(page.rooms[0].room_id, "room-1");
        assert_eq!(page.rooms[0].authority_epoch, 3);
        assert_eq!(page.rooms[0].members[1].handle, "@beta");
        assert_eq!(page.rooms[0].latest_seq, Some(12));
        assert_eq!(page.rooms[0].disbanded_at, None);
        assert_eq!(page.next_offset, Some(100));

        let empty: GroupsList = decode(json!({})).unwrap();
        assert_eq!(empty, GroupsList::default());
        assert!(decode::<GroupsList>(json!([1])).is_err());
    }

    #[test]
    fn decodes_capabilities_including_a_disabled_room_link() {
        let caps: GroupsCapabilities = decode(json!({
            "protocol_version": 1,
            "driver": true,
            "persistent_process": false,
            "authority_gateway_id": "gw-1",
            "room_link": {"enabled": false, "reason": "gateway_roomlink_secret_unavailable"},
            "features": ["authority_epoch", "log_replication"],
            "methods": ["groups.capabilities", "groups.list"],
            "max_log_limit": 1000
        }))
        .unwrap();
        assert!(caps.driver);
        assert!(!caps.room_link.enabled);
        assert_eq!(
            caps.room_link.reason.as_deref(),
            Some("gateway_roomlink_secret_unavailable")
        );
        assert!(caps.room_link.catalog.is_none());
        assert_eq!(caps.max_log_limit, 1000);

        let caps: GroupsCapabilities = decode(json!({
            "room_link": {
                "enabled": true, "profile": "swe",
                "catalog": {
                    "installation_id": "install-a", "protocol_versions": [1],
                    "link_modes": ["direct"], "persistent_process": true,
                    "text": true, "attachments": false,
                    "execution_policy": {
                        "version": 1, "target_profile": "swe",
                        "enabled_toolsets": ["files"], "approval_mode": "ask",
                        "max_iterations": 8, "policy_digest": "sha256:abc"
                    },
                    "catalog_digest": "sha256:def",
                    "endpoint": {"available": true, "url": "https://peer.example",
                                 "transport_security": "tls"}
                },
                "endpoint": {"available": true, "url": "https://peer.example",
                             "transport_security": "tls"}
            }
        }))
        .unwrap();
        let catalog = caps.room_link.catalog.unwrap();
        assert_eq!(catalog.execution_policy.max_iterations, 8);
        assert_eq!(catalog.endpoint.unwrap().transport_security.as_deref(), Some("tls"));

        assert_eq!(
            decode::<GroupsCapabilities>(json!({})).unwrap(),
            GroupsCapabilities::default()
        );
    }

    #[test]
    fn decodes_state_and_driver_status() {
        let state: GroupsState = decode(json!({
            "room": room(),
            "driver_status": {
                "running": true, "working": true, "blocked": false,
                "counts": {"queued": 1, "running": 2},
                "pending_actions": [{"kind": "retry", "task_id": "task-2"}],
                "peer_routes": [{"room_id": "room-1", "member_id": "beta", "status": "ready"}]
            }
        }))
        .unwrap();
        let driver = state.driver_status.unwrap();
        assert_eq!(driver.counts["running"], 2);
        assert_eq!(driver.pending_actions[0]["kind"], json!("retry"));
        assert_eq!(driver.peer_routes[0].status, "ready");

        let disbanded: GroupsState = decode(json!({"room": {"room_id": "r", "disbanded_at": 1.5}})).unwrap();
        assert!(disbanded.driver_status.is_none());
        assert_eq!(disbanded.room.disbanded_at, Some(1.5));
    }

    #[test]
    fn decodes_send_and_log() {
        let send: GroupsSend = decode(json!({
            "event": {
                "room_id": "room-1", "seq": 13, "event_id": "user:ev-7",
                "kind": "message.user", "actor": {"kind": "user", "id": "local"},
                "authority_epoch": 3, "payload": {"text": "hi"}, "created_at": 1700000100.0,
                "idempotent": true
            },
            "client_event_id": "ev-7", "accepted": true, "driver_started": true
        }))
        .unwrap();
        assert_eq!(send.event.seq, 13);
        assert_eq!(send.event.actor.id, "local");
        assert!(send.event.idempotent);
        assert_eq!(send.client_event_id.as_deref(), Some("ev-7"));

        let log: GroupsLog = decode(json!({
            "events": [], "cursor": 9, "latest_seq": 9, "has_more": false,
            "authority": {"gateway_id": "gw-1", "epoch": 3}
        }))
        .unwrap();
        assert_eq!(log.cursor, 9);
        assert!(!log.has_more);
        assert_eq!(log.authority.epoch, 3);

        assert_eq!(decode::<GroupsSend>(json!({})).unwrap(), GroupsSend::default());
        assert_eq!(decode::<GroupsLog>(json!({})).unwrap(), GroupsLog::default());
    }

    #[test]
    fn decodes_disband_stop_approve_retry() {
        let d: GroupsDisband = decode(json!({
            "tombstone": {"room_id": "room-1", "disbanded_at": 1700000200.0,
                          "idempotent": true, "history_expired": false}
        }))
        .unwrap();
        assert_eq!(d.tombstone.room_id, "room-1");
        assert!(d.tombstone.history_expired == Some(false));

        let s: GroupsStop = decode(json!({"cancelled": 2})).unwrap();
        assert_eq!(s.cancelled, 2);

        let a: GroupsApprove = decode(json!({"approved": true, "result": {"resolved": "once"}})).unwrap();
        assert_eq!(a.result["resolved"], json!("once"));

        let r: GroupsRetry = decode(json!({
            "retried": true,
            "task": {"room_id": "room-1", "task_id": "task-2", "thread_id": "t",
                     "turn_id": "u", "status": "queued", "execution_generation": 6,
                     "cancel_generation": 0}
        }))
        .unwrap();
        assert_eq!(r.task.status, "queued");
        assert_eq!(r.task.execution_generation, 6);

        assert_eq!(decode::<GroupsDisband>(json!({})).unwrap(), GroupsDisband::default());
        assert_eq!(decode::<GroupsStop>(json!({})).unwrap(), GroupsStop::default());
        assert_eq!(decode::<GroupsApprove>(json!({})).unwrap(), GroupsApprove::default());
        assert_eq!(decode::<GroupsRetry>(json!({})).unwrap(), GroupsRetry::default());
    }

    #[test]
    fn decodes_replication_shapes() {
        let rep: GroupsReplicate = decode(json!({
            "room_id": "room-1", "stored_seq": 12, "ingested": 5,
            "authority": {"gateway_id": "gw-1", "epoch": 3}, "caught_up": true
        }))
        .unwrap();
        assert_eq!(rep.ingested, 5);
        assert!(rep.caught_up);

        let repl: GroupsReplicaState = decode(json!({
            "room_id": "room-1", "name": "Planning", "members": [],
            "authority": {"gateway_id": "gw-1", "epoch": 3}, "last_seq": 12,
            "latest_seq": 14, "event_bytes": 2048,
            "created_at": 1700000000.0, "updated_at": 1700000300.0
        }))
        .unwrap();
        assert_eq!(repl.latest_seq, 14);

        let p: GroupsPromote = decode(json!({
            "room_id": "room-1", "authority_gateway_id": "gw-1", "authority_epoch": 4,
            "previous_gateway_id": "gw-0", "previous_epoch": 3, "claim_seq": 15,
            "latest_seq": 15
        }))
        .unwrap();
        assert_eq!(p.authority_epoch, 4);
        assert_eq!(p.previous_gateway_id, "gw-0");

        let d: GroupsDemote = decode(json!({
            "room_id": "room-1", "authority_gateway_id": "gw-0", "authority_epoch": 4,
            "idempotent": true
        }))
        .unwrap();
        assert!(d.idempotent);

        assert_eq!(
            decode::<GroupsReplicate>(json!({})).unwrap(),
            GroupsReplicate::default()
        );
        assert_eq!(
            decode::<GroupsReplicaState>(json!({})).unwrap(),
            GroupsReplicaState::default()
        );
        assert_eq!(decode::<GroupsPromote>(json!({})).unwrap(), GroupsPromote::default());
        assert_eq!(decode::<GroupsDemote>(json!({})).unwrap(), GroupsDemote::default());
    }

    #[test]
    fn decodes_peer_replies() {
        let inv: GroupsPeerInvite = decode(json!({
            "grant": "grant-token", "target_profile": "beta",
            "catalog": {"installation_id": "install-a", "protocol_versions": [1],
                        "link_modes": ["direct"], "catalog_digest": "sha256:abc"},
            "endpoint": {"available": true, "url": "https://peer.example",
                         "transport_security": "tls"}
        }))
        .unwrap();
        assert_eq!(inv.catalog.link_modes, vec!["direct".to_string()]);
        assert_eq!(inv.endpoint.url.as_deref(), Some("https://peer.example"));

        let reg: GroupsPeerRegister = decode(json!({
            "registered": true, "mode": "direct", "transport_security": "tls",
            "target_install_id": "install-b", "target_profile": "beta"
        }))
        .unwrap();
        assert_eq!(reg.mode, "direct");
        assert!(reg.registered);

        assert!(decode::<GroupsPeerRevoke>(json!({"revoked": true})).unwrap().revoked);
        assert_eq!(
            decode::<GroupsPeerInvite>(json!({})).unwrap(),
            GroupsPeerInvite::default()
        );
        assert_eq!(
            decode::<GroupsPeerRegister>(json!({})).unwrap(),
            GroupsPeerRegister::default()
        );
    }

    #[test]
    fn decodes_relay_replies() {
        let drain: BotRelayOutboxDrain = decode(json!({
            "envelopes": [{
                "id": "env-1", "created_at": 1700000000, "canonical_delivery_v1": true,
                "from_profile": "alpha", "from_handle": "@alpha",
                "target_connection": "conn-2", "target_profile": "beta",
                "target_handle": "@beta", "message": "hi"
            }]
        }))
        .unwrap();
        assert_eq!(drain.envelopes[0].target_connection, "conn-2");
        assert_eq!(drain.envelopes[0].created_at, 1700000000.0);

        let d: BotRelayDeliver = decode(json!({"reply": "pong"})).unwrap();
        assert_eq!(d.reply, "pong");
        let r: BotRelayReply = decode(json!({"ok": true})).unwrap();
        assert!(r.ok);
        let s: BotRelayRosterSync = decode(json!({"count": 3})).unwrap();
        assert_eq!(s.count, 3);

        assert_eq!(
            decode::<BotRelayOutboxDrain>(json!({})).unwrap(),
            BotRelayOutboxDrain::default()
        );
        assert_eq!(
            decode::<BotRelayDeliver>(json!({})).unwrap(),
            BotRelayDeliver::default()
        );
        assert_eq!(decode::<BotRelayReply>(json!({})).unwrap(), BotRelayReply::default());
        assert_eq!(
            decode::<BotRelayRosterSync>(json!({})).unwrap(),
            BotRelayRosterSync::default()
        );
    }

    #[test]
    fn outbox_pending_notification() {
        assert_eq!(BOT_RELAY_OUTBOX_PENDING, "bot_relay.outbox.pending");
        let p = parse_bot_relay_outbox_pending("bot_relay.outbox.pending", &json!({})).unwrap();
        assert_eq!(p, BotRelayOutboxPending::default());
        // A future field must not break the parse and must stay visible.
        let p = parse_bot_relay_outbox_pending("bot_relay.outbox.pending", &json!({"profile": "alpha", "count": 2}))
            .unwrap();
        assert_eq!(p.extra["profile"], json!("alpha"));
        assert!(parse_bot_relay_outbox_pending("sessions.changed", &json!({})).is_none());
    }
}
