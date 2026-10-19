//! Live checks for the GROUPS area (`groups.*`, `bot_relay.*`) against the PRIVATE test
//! backend.
//!
//! ```sh
//! HERMES_DESKTOP_PORT=<port> cargo test -p hermes-core --test live_api_groups -- --ignored --nocapture
//! ```
//!
//! # What runs live (local reads + harmless error paths only)
//! * `groups.capabilities` — protocol negotiation, no room work.
//! * `groups.list` — reads this gateway's room table.
//! * `groups.state`, `groups.log`, `groups.replica_state` — called with an unknown, tagged
//!   room id and asserted to answer a typed JSON-RPC server error (a read that cannot
//!   mutate anything because the room does not exist).
//!
//! # Builder-only (deliberately NOT called here)
//! * `groups.create` — wakes the room driver (`HostedRoomService.create_room` →
//!   `runtime.wakeup()`); creating a throwaway room could start member work.
//! * `groups.send`, `groups.approve`, `groups.retry` — append a user event / resume a run, so
//!   they spend model calls in the members' profiles.
//! * `groups.rename`, `groups.stop` — mutate the room log and cancel running work.
//! * `groups.disband` — permanently tombstones a room id.
//! * `groups.replicate`, `groups.promote`, `groups.demote` — cross-gateway authority
//!   movement; a wrong page/epoch corrupts a replica.
//! * `groups.peer.invite`, `groups.peer.revoke` — mint/revoke bearer grants for another
//!   installation.
//! * `groups.peer.register` — probes a peer gateway over HTTP (network).
//! * `bot_relay.roster.sync` — replaces this install's remote-agent roster.
//! * `bot_relay.outbox.drain` — claims (removes) real queued mail.
//! * `bot_relay.deliver` — runs a whole turn on another profile's Bot Chat (model calls,
//!   blocks up to 25 min).
//! * `bot_relay.reply` — resolves another connection's waiter.
//!
//! The `bot_relay.outbox.pending` notification payload is covered by unit tests; triggering it
//! live would require enqueueing a real relay envelope.

mod common;

use hermes_core::api::ApiError;
use hermes_core::api::groups;
use hermes_core::rpc::RpcError;

fn detail<T: std::fmt::Debug>(r: &Result<T, ApiError>) -> String {
    match r {
        Ok(v) => {
            let mut s = format!("{v:?}");
            if s.len() > 200 {
                s.truncate(200);
                s.push('…');
            }
            s
        }
        Err(e) => e.to_string(),
    }
}

fn server_code<T>(r: &Result<T, ApiError>) -> Option<i64> {
    match r {
        Err(ApiError::Rpc(RpcError::Server { code, .. })) => Some(*code),
        _ => None,
    }
}

#[tokio::test]
#[ignore = "needs a private test backend: HERMES_DESKTOP_PORT=<port>"]
async fn groups_reads_and_unknown_room_errors() {
    let (_rest, client) = common::backend().await;
    let mut checks = common::Checks::default();
    let missing = format!("missing-room-{}", common::tag());

    let caps = groups::groups_capabilities(&client, "").await;
    let ok = matches!(&caps, Ok(c) if c.protocol_version > 0 && !c.methods.is_empty());
    checks.check("groups.capabilities", ok, detail(&caps));
    if let Ok(c) = &caps {
        checks.check(
            "capabilities advertises groups.list",
            c.methods.iter().any(|m| m == "groups.list"),
            format!("methods={}", c.methods.len()),
        );
    }

    let list = groups::groups_list(&client, "", 10, 0, false).await;
    checks.ok("groups.list", &list);
    if let Ok(page) = &list {
        checks.check(
            "groups.list paging",
            page.rooms.len() < 10 || page.next_offset == Some(10),
            format!("rooms={} next_offset={:?}", page.rooms.len(), page.next_offset),
        );
    }

    let state = groups::groups_state(&client, "", &missing, false).await;
    checks.check(
        "groups.state(unknown) is a typed error",
        server_code(&state).is_some(),
        detail(&state),
    );

    let log = groups::groups_log(&client, "", &missing, 0, 10, false).await;
    checks.check(
        "groups.log(unknown) is a typed error",
        server_code(&log).is_some(),
        detail(&log),
    );

    let replica = groups::groups_replica_state(&client, "", &missing).await;
    checks.check(
        "groups.replica_state(unknown) is a typed error",
        server_code(&replica).is_some(),
        detail(&replica),
    );

    checks.finish();
}
