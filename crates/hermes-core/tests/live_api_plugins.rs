//! Live checks for the plugins & connectors area against a PRIVATE test backend only.
//!
//! ```sh
//! scripts/live-test-backend.sh                                   # prints HERMES_DESKTOP_PORT=<port>
//! HERMES_DESKTOP_PORT=<port> cargo test -p hermes-core --test live_api_plugins -- --ignored --nocapture
//! ```
//!
//! Only the safe endpoints run here: plugin/connector reads, the kanban board read, the
//! `.env` re-read, and the two no-operation connector calls (which must answer the
//! documented `unknown_operation` error). Nothing installs, toggles, connects, spends
//! model calls or touches an account; `plugins.catalog.lookup`, `connectors.tools`,
//! `connectors.connect` and `reload.mcp` are builder+unit-test only (network / spawns).

mod common;

use hermes_core::RpcError;
use hermes_core::api::plugins::*;
use hermes_core::http::HttpError;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a private test backend (HERMES_DESKTOP_PORT)"]
async fn live_plugins_and_connectors() {
    let (rest, client) = common::backend().await;
    let mut checks = common::Checks::default();

    // ── Installed plugins (reads) ───────────────────────────────────────────────
    let legacy = plugins_list(&client).await;
    checks.ok("plugins.list", &legacy);
    if let Ok(l) = &legacy {
        checks.check(
            "plugins.list rows",
            !l.plugins.is_empty(),
            format!(
                "{} plugin-manager entries, first={:?}",
                l.plugins.len(),
                l.plugins.first().map(|r| (&r.name, &r.version, r.enabled))
            ),
        );
    }

    let managed = plugins_manage(&client, &plugins_manage_list_call("")).await;
    checks.ok("plugins.manage list", &managed);
    if let Ok(m) = &managed {
        checks.check(
            "plugins.manage list counts add up",
            !m.plugins.is_empty() && m.user_count + m.bundled_count == m.plugins.len() as u64,
            format!(
                "{} rows ({} user / {} bundled)",
                m.plugins.len(),
                m.user_count,
                m.bundled_count
            ),
        );
        let first = m.plugins.first();
        checks.check(
            "plugins.manage row shape",
            first.is_some_and(|r| !r.name.is_empty() && !r.key.is_empty() && !r.status.is_empty()),
            format!(
                "{:?}",
                first.map(|r| (&r.key, &r.source, &r.status, r.portable, r.has_desktop_half))
            ),
        );
    }

    // ── REST: no /api/plugins collection (the reason this area is RPC-only) ─────
    let bare = rest.get("/api/plugins").await;
    checks.check(
        "REST /api/plugins is not a served collection",
        matches!(&bare, Err(HttpError::Status { status: 404, .. })),
        format!("{bare:?}"),
    );

    // ── The bundled kanban plugin's board (a real per-plugin route) ─────────────
    let board = kanban_board(&rest, "", false, "").await;
    checks.ok("kanban board", &board);
    if let Ok(b) = &board {
        checks.check(
            "kanban board columns",
            b.columns.iter().any(|c| c.name == "triage") && b.latest_event_id >= 0,
            format!(
                "{} columns, latest_event_id={}, {} assignees",
                b.columns.len(),
                b.latest_event_id,
                b.assignees.len()
            ),
        );
        checks.check(
            "kanban board include_archived adds the archived column",
            kanban_board(&rest, "", true, "")
                .await
                .is_ok_and(|a| a.columns.iter().any(|c| c.name == "archived")),
            "GET /api/plugins/kanban/board?include_archived=true".to_owned(),
        );
    }

    // ── Connectors (reads; the backend signs in with the member's identity) ─────
    let owner = ConnectorOwner::Account;

    let list = connectors_list(&client, &owner, "").await;
    checks.ok("connectors.list", &list);
    if let Ok(l) = &list {
        checks.check(
            "connectors.list rows",
            l.available && !l.connectors.is_empty(),
            format!(
                "available={}, {} connectors, first={:?}",
                l.available,
                l.connectors.len(),
                l.connectors.first().map(|r| (&r.connector, r.enabled, r.connected))
            ),
        );
    }

    let catalog = connectors_catalog(&client, "").await;
    checks.ok("connectors.catalog", &catalog);
    if let Ok(c) = &catalog {
        checks.check(
            "connectors.catalog rows",
            !c.connectors.is_empty(),
            format!(
                "{} catalog entries, first={:?}",
                c.connectors.len(),
                c.connectors.first().map(|r| (&r.slug, &r.category))
            ),
        );
    }

    let accounts = connectors_accounts(&client, None, "").await;
    checks.ok("connectors.accounts", &accounts);
    let filtered = connectors_accounts(&client, Some("notion"), "").await;
    checks.ok("connectors.accounts?connector=notion", &filtered);
    if let (Ok(all), Ok(only)) = (&accounts, &filtered) {
        checks.check(
            "connector filter is a subset",
            only.accounts.len() <= all.accounts.len() && only.accounts.iter().all(|a| a.connector == "notion"),
            format!("{} total, {} for notion", all.accounts.len(), only.accounts.len()),
        );
    }

    let policy = connectors_policy_get(&client, "").await;
    checks.ok("connectors.policy.get", &policy);
    if let Ok(p) = &policy {
        checks.check(
            "connectors.policy.get layers",
            !p.layers.is_empty() && !p.effective.mode.is_empty(),
            format!(
                "{} layers, effective mode={} rev={}",
                p.layers.len(),
                p.effective.mode,
                p.effective.revision
            ),
        );
    }

    // No operation is open for this member, so both calls must answer the documented
    // 4004 / unknown_operation error — a param + error-surface check with no side effects.
    let status = connectors_operation_status(&client, &owner, "no-such-op", "").await;
    checks.check(
        "connectors.operation.status unknown op -> 4004",
        matches!(
            &status,
            Err(hermes_core::api::ApiError::Rpc(RpcError::Server { code: 4004, .. }))
        ),
        format!("{status:?}"),
    );
    let wake = connectors_operation_wake(&client, &owner, "no-such-op", "").await;
    checks.check(
        "connectors.operation.wake unknown op -> 4004",
        matches!(
            &wake,
            Err(hermes_core::api::ApiError::Rpc(RpcError::Server { code: 4004, .. }))
        ),
        format!("{wake:?}"),
    );

    // ── Reloads ─────────────────────────────────────────────────────────────────
    // reload.env only re-reads the backend's own .env from disk (local config).
    let env = reload_env(&client, None, "").await;
    checks.ok("reload.env", &env);

    checks.finish();
}
