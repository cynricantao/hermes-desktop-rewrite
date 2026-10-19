# Groups API (`api::groups`)

Typed client for hosted Group Chats (`groups.*`) and the cross-connection bot relay
(`bot_relay.*`), plus the `bot_relay.outbox.pending` notification. UI-free; the controller maps
these into the Group Chat and Bot Chat screens.

Sources of truth: `tui_gateway/methods_groups.py`, `tui_gateway/methods_bot_relay.py`,
`gateway/hosted_rooms.py`, `gateway/hosted_room_replicas.py`, `gateway/hosted_room_peer.py`,
contracts `tui_gateway/contracts/groups_bot_relay.py`, and the Electron calls in
`apps/desktop/src/plugins/hermes-bots/{canonical-groups,relay}.ts`.

| Feature | Function | Backend call | Notes |
|---|---|---|---|
| Protocol negotiation | `groups_capabilities_call` / `groups_capabilities` | RPC `groups.capabilities` `{profile}` | Local read. `methods` is the wire list of every supported `groups.*` method; `driver` = the room worker is running; `persistent_process` = rooms survive the process; `room_link` (`enabled`/`reason`, `catalog`, `endpoint`) says whether cross-gateway RoomLink works here. `max_log_limit` bounds `groups.log`. |
| Room list | `groups_list_call` / `groups_list` | RPC `groups.list` `{profile, limit, offset, include_disbanded}` | Local read, most recently changed first. `next_offset` is `None` on the last page. |
| Create room | `groups_create_call` / `groups_create` | RPC `groups.create` `{profile, room_id, name, members}` | **Spawns the room driver** (`runtime.wakeup()`); a subsequent send spends model calls. Idempotent on the exact same `(name, members)`; a different state for the same id answers `4110`. Authority is always this gateway's install id. |
| Room state | `groups_state_call` / `groups_state` | RPC `groups.state` `{profile, room_id, include_disbanded}` | Local read. `driver_status` (`running`/`working`/`blocked`, `counts`, `pending_actions`, `peer_routes`) is omitted for a disbanded room or a stopped worker. |
| Send message | `groups_send_call` / `groups_send` | RPC `groups.send` `{profile, room_id, event_id?, payload}` | **Spends model calls**: appends `message.user` (actor server-owned) and wakes the driver so addressed members answer. `payload` is validated server-side (`validate_user_payload`); reusing an `event_id` is idempotent. |
| Rename | `groups_rename_call` / `groups_rename` | RPC `groups.rename` `{profile, room_id, event_id, name}` | Atomic with a typed replay event; `event_id` is required (idempotency key). Wakes the room runtime. |
| Read log | `groups_log_call` / `groups_log` | RPC `groups.log` `{profile, room_id, since_seq, limit, include_disbanded}` | Local read. Page bounded by count and bytes: `{events, cursor, latest_seq, has_more, authority}`; `since_seq` ahead of the log answers `4112`. |
| Disband | `groups_disband_call` / `groups_disband` | RPC `groups.disband` `{profile, room_id, cancel_id?}` | **Destructive**: stops the room, revokes peer routes, tombstones the id forever (`4113` maps a room error; a room owned by another gateway answers `AuthorityConflict`). |
| Stop work | `groups_stop_call` / `groups_stop` | RPC `groups.stop` `{profile, room_id, cancel_id?}` | **Aborts in-flight turns**; reply `{cancelled}`. |
| Approve | `groups_approve_call` / `groups_approve` | RPC `groups.approve` `{profile, room_id, member_id, task_id, execution_generation, choice, request_id}` | **Resumes agent work** (approved run continues, spends model calls); a peer member's approval round-trips to its gateway. `Approval` carries the exact tuple from `driver_status.pending_actions`; a stale `execution_generation` is refused. |
| Retry | `groups_retry_call` / `groups_retry` | RPC `groups.retry` `{profile, room_id, task_id}` | **Spends model calls**: re-queues an indeterminate task after an explicit user confirmation. Reply `{retried, task: RoomTaskReceipt}`. |
| Replicate page | `groups_replicate_call` / `groups_replicate` | RPC `groups.replicate` `{profile, room_id, room_name, members, page}` | **Cross-gateway**: persists one authority-stamped `groups.log` page into the local replica store; idempotent, refuses gaps/epoch regressions. |
| Replica state | `groups_replica_state_call` / `groups_replica_state` | RPC `groups.replica_state` `{profile, room_id}` | Local read of the replica store (`last_seq` … `latest_seq`, `authority`). Error `4117` when no replica exists. |
| Promote | `groups_promote_call` / `groups_promote` | RPC `groups.promote` `{profile, room_id, confirm: true, reason}` | **Split-brain risk**: continues the room here at `epoch + 1`. The builder always sends `confirm: true`; only call when the previous authority provably cannot commit. |
| Demote | `groups_demote_call` / `groups_demote` | RPC `groups.demote` `{profile, room_id, observed_gateway_id, observed_epoch}` | **Fences this gateway** against a newer epoch; only after a peer proved authority. |
| Peer invite | `groups_peer_invite_call` / `groups_peer_invite` | RPC `groups.peer.invite` `{profile, room_id, home_install_id, authority_gateway_id, authority_epoch, member_id, grant_id?, ttl_seconds?}` | **Network / bearer token**: mints a grant authorizing another installation's profile to run this member's turns (60 s … 86400 s, default 3600). Never log `grant`. |
| Peer revoke | `groups_peer_revoke_call` / `groups_peer_revoke` | RPC `groups.peer.revoke` `{profile, grant}` | Revokes only grants this gateway issued; scope mismatch → `4122`. |
| Peer register | `groups_peer_register_call` / `groups_peer_register` | RPC `groups.peer.register` `{profile, room_id, member_id, target_url, target_profile, grant, catalog, cancellation_scope_id?, trace_id?}` | **Network**: validates and probes `target_url` over HTTP with the grant, then installs the route. Catalog/scope mismatch → `5120`; missing worker → `4121`. |
| Relay roster | `bot_relay_roster_sync_call` / `bot_relay_roster_sync` | RPC `bot_relay.roster.sync` `{agents}` | Install-wide (no `profile`). **Replaces** the gateway's remote-agent roster; rows are free-form (`{profile, handle, connection_id, connection_label?, title?, description?, online?}`), invalid rows are dropped. Reply `{count}`. |
| Outbox drain | `bot_relay_outbox_drain_call` / `bot_relay_outbox_drain` | RPC `bot_relay.outbox.drain` `{}` | **Claims mail**: envelopes move to `claimed/` atomically; whoever drains must deliver them. Reply `{envelopes: RelayEnvelope[]}`. |
| Deliver | `bot_relay_deliver_call` / `bot_relay_deliver` | RPC `bot_relay.deliver` `{profile, message, id, from_profile?, from_handle?, from_connection?}` | **Spends model calls, blocks a whole turn** (timeout `RELAY_DELIVER_TIMEOUT` = 1500 s, desktop parity). `profile` is the *target* profile here; `id` must be 32–64 lowercase hex (`4090`), unknown profile `4092`, unreachable runtime `5094`/`runtime_unavailable` (the sender retries). |
| Relay reply | `bot_relay_reply_call` / `bot_relay_reply` | RPC `bot_relay.reply` `{id, reply, error, reason}` | Resolves the sender-side waiter. **External side effect**; empty id `4093`, unknown/settled envelope `4094`. |
| Outbox signal | `BOT_RELAY_OUTBOX_PENDING`, `parse_bot_relay_outbox_pending` | notification `bot_relay.outbox.pending` | Payload is `{}` today (`ChangeSignalPayload`); `BotRelayOutboxPending.extra` keeps unknown keys. Reaction: `bot_relay_outbox_drain`. |

Response types (`GroupsCapabilities`, `GroupsList`, `GroupsState`, `GroupsSend`, `GroupsLog`,
`GroupsDisband`, `GroupsStop`, `GroupsApprove`, `GroupsRetry`, `GroupsReplicate`,
`GroupsReplicaState`, `GroupsPromote`, `GroupsDemote`, `GroupsPeerInvite`,
`GroupsPeerRevoke`, `GroupsPeerRegister`, `Room`, `RoomEvent`, `RoomMember`, `RoomAuthority`,
`RoomTombstone`, `RoomDriverStatus`, `RoomTaskReceipt`, `RoomLinkCatalog`,
`RoomLinkStatus`, `BotRelayRosterSync`, `BotRelayOutboxDrain`, `RelayEnvelope`,
`BotRelayDeliver`, `BotRelayReply`) are lenient: `#[serde(default)]`, so a missing or extra
field never fails a decode.

Helpers/builder inputs: `RoomMemberInput` (create/replicate roster rows),
`PeerInvite`, `PeerRegister`, `Approval`, `RELAY_DELIVER_TIMEOUT`.

Every `groups.*` builder sends `profile` (`""` = the gateway's own profile). The relay methods
are install-wide and send only the fields above; `bot_relay.deliver`'s `profile` is the target.

## Run live (private test backend)

`crates/hermes-core/tests/live_api_groups.rs` — 7 checks, against
`HERMES_DESKTOP_PORT=<port>` (`scripts/live-test-backend.sh`):

* RPC `groups.capabilities` (decodes; `protocol_version > 0`, `methods` non-empty and contains
  `groups.list`)
* RPC `groups.list` `{limit: 10, offset: 0}` (decodes; paging consistent)
* RPC `groups.state` / `groups.log` / `groups.replica_state` with an unknown tagged room id →
  typed server errors `4114` / `4112` / `4117`

## Not run live (and why)

| Endpoint | Why not |
|---|---|
| `groups.create` | Wakes the room driver; a throwaway room could start member work. |
| `groups.send`, `groups.approve`, `groups.retry` | Spend model calls / resume agent runs in a member's profile. |
| `groups.rename`, `groups.stop`, `groups.disband` | Mutate the room log / cancel work / tombstone the id. |
| `groups.replicate`, `groups.promote`, `groups.demote` | Cross-gateway authority; needs a second gateway and can corrupt a replica. |
| `groups.peer.invite`, `groups.peer.revoke`, `groups.peer.register` | Bearer grants and an HTTP probe of another installation. |
| `bot_relay.roster.sync` | Replaces this install's remote-agent roster (shared state). |
| `bot_relay.outbox.drain` | Claims real queued mail. |
| `bot_relay.deliver` | Runs a whole turn on another profile's Bot Chat (model calls). |
| `bot_relay.reply` | Resolves another connection's waiter. |
| `bot_relay.outbox.pending` | Emitting it requires enqueueing a real relay envelope; the payload parse is unit-tested. |
