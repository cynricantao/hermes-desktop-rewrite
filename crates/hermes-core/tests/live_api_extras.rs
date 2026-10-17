//! Live checks for the capability extras REST + RPC wrappers (skills hub, tools/toolsets,
//! terminal backends, computer use, MCP, learning, vault) against a PRIVATE test backend.
//!
//! ```sh
//! HERMES_DESKTOP_PORT=37273 cargo test -p hermes-core --test live_api_extras -- --ignored --nocapture
//! ```
//!
//! Read-only endpoints plus one reversible MCP add/remove are exercised. Network-bound,
//! model-bound, process-spawning and secret-bearing endpoints are builder/unit-tested only.

mod common;

use hermes_core::api::extras::*;
use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a private test backend (HERMES_DESKTOP_PORT)"]
async fn extras_rest_and_rpc() {
    let (rest, client) = common::backend().await;
    let mut checks = common::Checks::default();

    // ── Skills hub (local catalog scan + local registry rescan) ──
    let official = skills_hub_official(&rest, "").await;
    if checks.ok("skills hub official", &official) {
        let o = official.unwrap();
        checks.check(
            "official catalog rows",
            o.skills.iter().all(|s| !s.name.is_empty() && !s.identifier.is_empty()),
            format!("{} skills", o.skills.len()),
        );
    }
    checks.ok("skills.reload", &skills_reload(&client, "", "").await);
    checks.ok("skills.manage list", &skills_manage(&client, "list", "", "").await);

    // ── Tools / toolsets (RPC reads + one category-less config panel) ──
    let toolsets = toolsets_list(&client, "").await;
    let mut have_file = false;
    if checks.ok("toolsets.list", &toolsets) {
        let ts = toolsets.unwrap();
        checks.check(
            "toolsets.list non-empty",
            !ts.toolsets.is_empty(),
            format!("{} toolsets", ts.toolsets.len()),
        );
        have_file = ts.toolsets.iter().any(|t| t.name == "file");
    }
    let tools = tools_list(&client, "").await;
    if checks.ok("tools.list", &tools) {
        let t = tools.unwrap();
        checks.check(
            "tools.list carries tool names",
            t.toolsets.iter().any(|r| !r.tools.is_empty()),
            format!("{} toolsets", t.toolsets.len()),
        );
    }
    let show = tools_show(&client, "").await;
    if checks.ok("tools.show", &show) {
        let s = show.unwrap();
        checks.check(
            "tools.show sections",
            s.total >= 0 && s.sections.iter().all(|x| !x.name.is_empty()),
            format!("{} sections, {} tools", s.sections.len(), s.total),
        );
    }
    if have_file {
        // Category-less toolsets do not contact the Nous portal, so this is a local read.
        let cfg = toolset_config(&rest, "file", "").await;
        if checks.ok("toolset config (file)", &cfg) {
            let c = cfg.unwrap();
            checks.check(
                "file toolset has no provider category",
                !c.has_category && c.providers.is_empty(),
                format!("active_provider={:?}", c.active_provider),
            );
        }
    } else {
        println!("[SKIP] toolset config: no category-less 'file' toolset on this backend");
    }

    // ── Terminal backends (health probes are part of the read) ──
    let backends = terminal_backends(&rest, "").await;
    if checks.ok("terminal backends", &backends) {
        let b = backends.unwrap();
        checks.check(
            "terminal backends listing",
            !b.backends.is_empty() && b.backends.iter().any(|x| x.active),
            format!("active={} ({})", b.active, b.backends.len()),
        );
        // Selecting the already-active backend proves the PUT without changing state.
        let active = b.active.clone();
        let selected = select_terminal_backend(&rest, &active, "").await;
        if checks.ok("terminal backend select (no-op)", &selected) {
            checks.check(
                "terminal backend echoes the selection",
                selected.unwrap().backend == active,
                active,
            );
        }
    }

    // ── Computer use ──
    let cu = computer_use_status(&rest, "").await;
    if checks.ok("computer use status", &cu) {
        let c = cu.unwrap();
        checks.check(
            "computer use platform reported",
            !c.platform.is_empty(),
            format!("platform={} ready={}", c.platform, c.ready),
        );
    }

    // ── MCP: REST catalog, RPC catalog, server reads, reversible add/remove ──
    if checks.ok("mcp REST catalog", &mcp_catalog(&rest, "").await) {
        // entries may legitimately be empty in an isolated profile; diagnostics still decode.
    }
    if checks.ok("mcp.catalog RPC", &mcp_catalog_rpc(&client, "").await) {
        // per-profile preset state; content is backend-owned.
    }
    checks.ok("mcp.servers.list", &mcp_servers_list(&client, "").await);
    checks.ok("mcp.servers.status", &mcp_servers_status(&client, "").await);

    let name = format!("api-extras-{}", common::tag());
    let added = mcp_servers_add(&client, &name, "", Some(json!({"command": "echo"})), "").await;
    if checks.ok("mcp.servers.add (throwaway)", &added) {
        let listed = mcp_servers_list(&client, "").await.unwrap_or_default();
        checks.check(
            "added MCP server is listed",
            listed.servers.iter().any(|s| s.name == name),
            name.clone(),
        );
        checks.ok(
            "mcp.servers.remove (cleanup)",
            &mcp_servers_remove(&client, &name, "").await,
        );
        let after = mcp_servers_list(&client, "").await.unwrap_or_default();
        checks.check(
            "removed MCP server is gone",
            !after.servers.iter().any(|s| s.name == name),
            name,
        );
    }

    // ── Learning graph / node / frames ──
    let graph = learning_graph(&rest, "").await;
    let mut node_id = None;
    if checks.ok("learning graph", &graph) {
        let g = graph.unwrap();
        checks.check(
            "learning graph decodes",
            g.edges.iter().all(|e| !e.source.is_empty() && !e.target.is_empty()),
            format!("{} nodes, {} edges", g.nodes.len(), g.edges.len()),
        );
        node_id = g.nodes.first().map(|n| n.id.clone());
    }
    if let Some(id) = node_id {
        checks.ok("learning node", &learning_node(&rest, &id, "").await);
    } else {
        println!("[SKIP] learning node: the graph has no nodes");
    }
    checks.ok("learning.frames", &learning_frames(&client, 60, 20, 4).await);

    // ── Vault (metadata reads only) ──
    checks.ok("vault.list", &vault_list(&client, "").await);
    if checks.ok("vault.sources", &vault_sources(&client, "").await) {
        // source rows are backend-owned; decoding is the assertion.
    }

    // ── Browser (CDP) control — `status` is a local read; the other actions write
    //    backend state / spawn supervisors and stay builder-only ──
    let browser = browser_status(&client, "").await;
    if checks.ok("browser.manage status", &browser) {
        let b = browser.unwrap();
        // The handler pairs `connected` with a non-empty url (`bool(_resolve_browser_cdp_url())`);
        // no CDP override answers `url: ""`.
        checks.check(
            "browser status shape",
            b.connected == b.url.as_deref().is_some_and(|u| !u.is_empty()),
            format!(
                "connected={} url={:?} browser_use={}",
                b.connected, b.url, b.browser_use
            ),
        );
    }

    checks.finish();
}
