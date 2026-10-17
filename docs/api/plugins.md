# Plugins & connectors — `crates/hermes-core/src/api/plugins.rs`

Typed Rust wrappers for the plugin manager (`plugins.*`), the per-plugin REST namespace
(`/api/plugins/<id>/…`), the connectors page (`connectors.*`) and the env/MCP reloads.
All RPC wrappers take an optional `profile` (empty string = the launch profile, sent as
*omitted*); every response struct is lenient (`#[serde(default)]`, unknown fields ignored).
Live coverage is in `crates/hermes-core/tests/live_api_plugins.rs` (safe reads only).

| Feature | Function | Backend call | Notes |
|---|---|---|---|
| Installed plugins (legacy flat view) | `plugins_list_call` / `plugins_list` | RPC `plugins.list` `{}` | Plugin-manager rows `{name, version, enabled}`. |
| Plugins Hub — list | `plugins_manage_list_call` | RPC `plugins.manage` `{action:"list"}` | `PluginsManageResult.plugins: Vec<PluginRow>` — canonical `key`, `source`, `status`, `servers`, `settings_schema`, catalog pins (`installed_sha`, `catalog_sha`, `update_available`, `pinned_sha`). |
| Plugins Hub — enable/disable | `plugins_manage_toggle_call` | RPC `plugins.manage` `{action:"toggle", key, enable}` | Toggles by canonical `key`; reply carries `activation` + `gateway_reloaded`. |
| Plugins Hub — install | `plugins_manage_install_call` | RPC `plugins.manage` `{action:"install", identifier \| catalog_name, force, enable, ref?}` | **Network** (git clone) + writes `<HERMES_HOME>/plugins/`. |
| Plugins Hub — update | `plugins_manage_update_call` | RPC `plugins.manage` `{action:"update", name, accept_capabilities?}` | **Network**; a re-pin that widens capabilities answers `consent_required` + `delta` first. |
| Plugins Hub — remove | `plugins_manage_remove_call` | RPC `plugins.manage` `{action:"remove", name}` | **Destructive** — deletes the user install directory. |
| Plugins Hub — settings | `plugins_manage_settings_call` | RPC `plugins.manage` `{action:"settings", key, values}` | Writes manifest `config_schema` values (non-secret keys; secrets go through the `.env` route). |
| Onboarding card | `plugins_manage_onboarding_call` | RPC `plugins.manage` `{action:"onboarding"}` | **Network** — reads the live catalog (`onboarding[]`). |
| Presence probe | `plugins_manage_presence_call` | RPC `plugins.manage` `{action:"presence", names}` | **Network** — `presence[]` in the order asked. |
| Run any manage action | `plugins_manage` | the prepared `Call` | Single executor for the eight builders above. |
| Catalog lookup | `plugins_catalog_lookup_call` / `plugins_catalog_lookup` | RPC `plugins.catalog.lookup` `{name}` | **Network**; `status` is `found` / `invalid_name` / `unknown` / `unavailable`. |
| Per-plugin REST namespace | `plugin_rest_request` / `plugin_rest` | REST `/api/plugins/<id>/<suffix>` | The desktop's `pluginRest` door. Rejects `..` path segments (desktop parity); reply is the plugin's own JSON — use `plugin_rest(..) -> Value` or add a typed decode. |
| Kanban board | `kanban_board_request` / `kanban_board` | REST `GET /api/plugins/kanban/board?board=&include_archived=` | Typed `KanbanBoard` (columns / tenants / assignees / `latest_event_id`); always pass `board` so no request flips the server-wide board pointer. |
| Connectors — list | `connectors_list_call` / `connectors_list` | RPC `connectors.list` `{owner}` | **Network**; owner = `ConnectorOwner::Account` or `::session(id)`. |
| Connectors — catalog | `connectors_catalog_call` / `connectors_catalog` | RPC `connectors.catalog` `{}` | **Network** — hosted directory. |
| Connectors — connect | `connectors_connect_call` / `connectors_connect` | RPC `connectors.connect` `{owner, connectors, reconnect, alias?, connection_id?}` | **Network** — opens an authorization operation (`ConnectorsConnectResult`). |
| Connectors — accounts | `connectors_accounts_call` / `connectors_accounts` | RPC `connectors.accounts` `{connector?}` | **Network**; optional connector filter. |
| Connectors — account remove | `connectors_accounts_remove_call` / `connectors_accounts_remove` | RPC `connectors.accounts.remove` `{connection_id}` | **Network**, **Destructive**. |
| Connectors — account rename | `connectors_accounts_rename_call` / `connectors_accounts_rename` | RPC `connectors.accounts.rename` `{connection_id, alias}` | **Network**; returns the updated `ConnectorAccountRow`. |
| Connectors — tools | `connectors_tools_call` / `connectors_tools` | RPC `connectors.tools` `{slug, refresh}` | **Network**; `refresh: true` re-fetches from the hosted service, otherwise the cache. |
| Connectors — policy get | `connectors_policy_get_call` / `connectors_policy_get` | RPC `connectors.policy.get` `{}` | **Network**; layers (org → role → member) + `effective`. |
| Connectors — policy set | `connectors_policy_set_call` / `connectors_policy_set` | RPC `connectors.policy.set` `{change, expected_revision}` | **Network** write; `ConnectorPolicyChange::Tools{v..}` / `::Connector{v..}`; stale revision → `policy_conflict`. |
| Connectors — op status | `connectors_operation_status_call` / `connectors_operation_status` | RPC `connectors.operation.status` `{owner, op_id}` | **Network**; unknown op → 4004 `unknown_operation`. |
| Connectors — op wake | `connectors_operation_wake_call` / `connectors_operation_wake` | RPC `connectors.operation.wake` `{owner, op_id}` | **Network**; unknown op → 4004. |
| Env reload | `reload_env_call` / `reload_env` | RPC `reload.env` `{session_id?}` | Re-reads the backend's own `.env` from disk; returns `{updated}`. |
| MCP reload | `reload_mcp_call` / `reload_mcp` | RPC `reload.mcp` `{session_id?, confirm?, always?, rev?}` | **Spawns processes** (stdio MCP servers reconnect) and invalidates prompt caches; without `confirm` answers `confirm_required`. |

## Live checks — `crates/hermes-core/tests/live_api_plugins.rs`

Runs only against a private `scripts/live-test-backend.sh` backend
(`HERMES_DESKTOP_PORT=<port>`), never the everyday one. Last run against the isolated
test backend on port 37273: **21 checks, 0 failed** (`test live_plugins_and_connectors ... ok`).

* `plugins.list` — rows present.
* `plugins.manage` `list` — rows present, `user_count + bundled_count == plugins.len()`,
  first row has non-empty `name` / `key` / `status`.
* `REST /api/plugins` — asserted **not** a served collection (404).
* `GET /api/plugins/kanban/board` — columns include `triage`; `include_archived=true`
  adds the `archived` column.
* `connectors.list` / `connectors.catalog` / `connectors.accounts` (all + `?connector=notion`
  subset check) / `connectors.policy.get` — rows / layers present.
* `connectors.operation.status` and `.wake` with a bogus `op_id` — must answer the
  documented RPC error 4004 `unknown_operation` (no side effects).
* `reload.env` — re-reads the test backend's own `.env`.

## Not run live (and why)

| Function | Why not live |
|---|---|
| `plugins_manage_toggle_call` | Mutates plugin enablement (and re-wires the gateway). Reversible but a state change; builder + unit-tested only. |
| `plugins_manage_install_call`, `plugins_manage_update_call` | **Network** (clones from git) and writes to disk. |
| `plugins_manage_remove_call` | **Destructive** — would have to create then delete a user install; out of the safe set. |
| `plugins_manage_settings_call` | Writes plugin config to disk. |
| `plugins_manage_onboarding_call`, `plugins_manage_presence_call`, `plugins_catalog_lookup_call` | **Network** (live catalog feed). |
| `connectors_tools_call` (refresh) | **Network**; no hosted toolkit fetch is allowed in the test run. |
| `connectors_connect_call` | **Network** — opens an OAuth/authorization operation and a browser leg. |
| `connectors_accounts_remove_call`, `connectors_accounts_rename_call` | **Network** + would act on a real hosted account (none exists for the test member; removing/renaming is destructive). |
| `connectors_policy_set_call` | **Network** write to the member policy. |
| `reload_mcp_call` | **Spawns processes** — MCP servers are torn down and rediscovered. |
| `plugin_rest_request` / `plugin_rest` (generic) | Covered indirectly: the kanban board check goes through the same namespace builder. Other plugin sub-routes are plugin-specific and not typed here. |

## Not found / not implemented

* **No REST `/api/plugins` collection.** There is no list / get / enable / delete REST route;
  `hermes serve` answers 404 for `/api/plugins` (verified live), and the dashboard mounts only
  one forwarded router per plugin (`/api/plugins/<name>/{path:path}`). Plugin management is
  RPC-only (`plugins.manage`), so no REST CRUD model is typed here.
* No REST route for connector accounts/policy either — those are RPC-only (`connectors.*`).
* The generic per-plugin namespace is served for whatever plugins are installed; only the
  bundled kanban plugin's `board` route is typed (the desktop uses it directly), everything
  else goes through `plugin_rest(..) -> Value`.
