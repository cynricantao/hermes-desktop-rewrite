# Capability extras — typed API (part A)

Skills hub, tools/toolsets, terminal backends, computer use, MCP (RPCs + catalog), learning
graph and the credential vault. Area module: `crates/hermes-core/src/api/extras.rs`.
Cron/profile/memory-provider extras live in `extras::admin` (`docs/api/extras-admin.md`).

Conventions are those of [`crate::api`](../../crates/hermes-core/src/api/mod.rs): every endpoint
has a pure builder returning `Request`/`Call` plus an async wrapper, and every response struct
is `#[serde(default)]` so older/newer backends never fail to decode.

## Skills hub (REST)

| Feature | Function | Backend call | Notes |
|---|---|---|---|
| Official catalog | `skills_hub_official` | `GET /api/skills/hub/official[?profile=]` | Local scan of the optional-skills catalog; rows carry `category` + `installed` |
| Hub sources | `skills_hub_sources` | `GET /api/skills/hub/sources[?profile=]` | **Network**; sources, `index_available`, featured skills, installed lock map |
| Search | `skills_hub_search` | `GET /api/skills/hub/search?q=&source=&limit=[&profile=]` | **Network**; `limit` capped at 50 (0 = backend default 20), empty `q` returns nothing |
| Preview | `skills_hub_preview` | `GET /api/skills/hub/preview?identifier=[&profile=]` | **Network**; SKILL.md + file list, 404 when unknown |
| Security scan | `skills_hub_scan` | `GET /api/skills/hub/scan?identifier=[&profile=]` | **Network**; verdict/policy/findings + optional Tier-1 block |
| Install | `skills_hub_install` | `POST /api/skills/hub/install[?profile=]` `{identifier}` | **Spawns a process / network**; returns `{ok, pid, name}` |
| Uninstall | `skills_hub_uninstall` | `POST /api/skills/hub/uninstall[?profile=]` `{name}` | **Spawns a process / destructive** |
| Update all | `skills_hub_update` | `POST /api/skills/hub/update[?profile=]` | **Spawns a process / network** |

### Skills RPCs

| Feature | Function | Backend call | Notes |
|---|---|---|---|
| Skills actions | `skills_manage` | RPC `skills.manage` `{action, query[, profile]}` | `list`/`search`/`browse`/`inspect`/`install`; reply shape depends on action, returned as `Value` |
| Registry rescan | `skills_reload` | RPC `skills.reload` `{session_id?, profile?}` | Local disk rescan; `output` log + `result{added, removed, total}` |

## Tools / toolsets (REST config panels)

| Feature | Function | Backend call | Notes |
|---|---|---|---|
| Toolset config | `toolset_config` | `GET /api/tools/toolsets/{name}/config[?profile=]` | Provider matrix + env-key status. A toolset **with** a provider category contacts the Nous portal (`force_fresh`) → **network**; category-less toolsets (`file`, `terminal`, …) are local reads |
| Model catalog | `toolset_models` | `GET /api/tools/toolsets/{name}/models?provider=[&profile=]` | Image/video generation only; `has_models: false` otherwise. **Network** (entitlement probe) |
| Select model | `toolset_select_model` | `PUT /api/tools/toolsets/{name}/model[?profile=]` `{model[, provider]}` | Persists the model; reply passed through |
| Select provider | `toolset_select_provider` | `PUT /api/tools/toolsets/{name}/provider[?profile=]` `{provider[, capability]}` | `capability` = `search`/`extract` for web only |
| Save env | `toolset_save_env` | `PUT /api/tools/toolsets/{name}/env[?profile=]` `{env}` | **Reveals secrets** (credentials in the body) |
| Post-setup | `toolset_post_setup` | `POST /api/tools/toolsets/{name}/post-setup[?profile=]` `{key}` | **Spawns a process** (pip/apt/…); reply passed through |

### Tools RPCs

| Feature | Function | Backend call | Notes |
|---|---|---|---|
| Toolsets + tools | `tools_list` | RPC `tools.list` `{session_id?}` | Rows with `tool_count`, `enabled`, resolved tool names |
| Toolsets | `toolsets_list` | RPC `toolsets.list` `{session_id?}` | Same rows without the tool names |
| All tools | `tools_show` | RPC `tools.show` `{session_id?}` | `{sections: [{name, tools: [{name, description}]}], total}` |
| Configure tools | `tools_configure` | RPC `tools.configure` `{action, names[, session_id]}` | **Writes config**; `action` = `enable`/`disable`, names are toolsets or `server:tool`; `reset: true` means a live agent was rebuilt |

## Terminal backends

| Feature | Function | Backend call | Notes |
|---|---|---|---|
| Backends | `terminal_backends` | `GET /api/tools/terminal/backends[?profile=]` | **Spawns health probes** (docker/ssh/…); failures are a `status`, not an error |
| Select backend | `select_terminal_backend` | `PUT /api/tools/terminal/backend[?profile=]` `{backend}` | Persists `terminal.backend`; a backend needing setup is allowed |

## Computer use

| Feature | Function | Backend call | Notes |
|---|---|---|---|
| Status | `computer_use_status` | `GET /api/tools/computer-use/status[?profile=]` | `platform`, `ready`, `can_grant`, per-permission `checks` |
| Grant permissions | `grant_computer_use_permissions` | `POST /api/tools/computer-use/permissions/grant[?profile=]` | **Spawns a process**; macOS only (400 elsewhere) |

## MCP

| Feature | Function | Backend call | Notes |
|---|---|---|---|
| Catalog | `mcp_catalog` | `GET /api/mcp/catalog[?profile=]` | Local manifests + diagnostics; entries carry `required_env` rows, transport details, `needs_install`, installed/enabled |
| Install preset | `mcp_catalog_install` | `POST /api/mcp/catalog/install[?profile=]` `{name, env, enable}` | **Spawns a process / network**; writes declared secrets to `.env` |
| Replace servers | `mcp_servers_replace` | `PUT /api/mcp/servers[?profile=]` `{servers}` | **Destructive** whole-map replace (mcp.json editor save); omitted servers are deleted |
| Server list | `mcp_servers_list` | RPC `mcp.servers.list` `{profile?}` | Secrets reduced to env-key names; `source` = config/plugin |
| Runtime status | `mcp_servers_status` | RPC `mcp.servers.status` `{profile?}` | Cached only — never connects or probes; `checked_at` in ms |
| Add server | `mcp_servers_add` | RPC `mcp.servers.add` `{name[, preset][, config][, profile]}` | **Writes config**; duplicate → 4090, suspicious command rejected |
| Remove server | `mcp_servers_remove` | RPC `mcp.servers.remove` `{name[, profile]}` | **Destructive** |
| Set API key | `mcp_servers_set_api_key` | RPC `mcp.servers.set_api_key` `{name, value[, env_var][, profile]}` | **Reveals secrets**; only a `${ENV}` reference persists in config |
| Probe server | `mcp_servers_test` | RPC `mcp.servers.test` `{name[, profile]}` | **Spawns a process / network**; cold `npx` can block |
| OAuth start | `mcp_servers_oauth_start` | RPC `mcp.servers.oauth.start` `{name[, client_redirect_uri][, profile]}` | **Network**; returns `auth_url` to open |
| OAuth poll | `mcp_servers_oauth_poll` | RPC `mcp.servers.oauth.poll` `{name, session_id[, profile]}` | **Network**; status `pending`/`approved`/`error` |
| OAuth cancel | `mcp_servers_oauth_cancel` | RPC `mcp.servers.oauth.cancel` `{name, session_id[, profile]}` | Wakes the callback worker; raw reply |
| OAuth callback | `mcp_servers_oauth_callback` | RPC `mcp.servers.oauth.callback` `{name, session_id[, code][, state][, profile]}` | Relays a client-captured redirect; raw reply |
| Catalog RPC | `mcp_catalog_rpc` | RPC `mcp.catalog` `{profile?}` | Per-profile preset state; required env keys come as `requires` (different shape from the REST catalog) |

## Learning graph (`/journey`)

| Feature | Function | Backend call | Notes |
|---|---|---|---|
| Graph | `learning_graph` | `GET /api/learning/graph[?profile=]` | Nodes (`useCount`, `createdBy`, `memorySource` camelCase) + edges; clusters/memory/stats kept as `Value` |
| Node detail | `learning_node` | `GET /api/learning/node?id=[&profile=]` | Content for an edit prefill; 404 when unknown |
| Edit node | `learning_node_update` | `PUT /api/learning/node[?profile=]` `{id, content}` | **Writes user data** (SKILL.md or memory chunk) |
| Delete node | `learning_node_delete` | `DELETE /api/learning/node[?profile=]` `{id}` | **Destructive** (skill archived, memory removed) |
| Frames | `learning_frames` | RPC `learning.frames` `{cols, rows, frames}` | Pre-rendered TUI timeline; frame grids are renderer-owned JSON |
| Detail RPC | `learning_detail` | RPC `learning.detail` `{id}` | Same payload as the REST node route |
| Edit RPC | `learning_edit` | RPC `learning.edit` `{id, content}` | **Writes user data** |
| Delete RPC | `learning_delete` | RPC `learning.delete` `{id}` | **Destructive**; `{ok, message?}` |

## Vault

| Feature | Function | Backend call | Notes |
|---|---|---|---|
| List | `vault_list` | RPC `vault.list` `{profile?}` | Metadata only across unlocked backends (`backend` per item); passwords never returned |
| Sources | `vault_sources` | RPC `vault.sources` `{profile?}` | `{name, display_name, enabled, needs_unlock, unlocked, installed}` |
| Add | `vault_add` | RPC `vault.add` `{kind, label[, origin], secret[, profile]}` | **Reveals secrets**; returns `{id}` |
| Remove | `vault_remove` | RPC `vault.remove` `{id[, profile]}` | **Destructive**; `{removed}` |
| Unlock | `vault_unlock` | RPC `vault.unlock` `{name, password[, profile]}` | **Reveals secrets** (master password) |
| Lock | `vault_lock` | RPC `vault.lock` `{name?[, profile]}` | Empty `name` locks every manager |
| Source toggle | `vault_source_set` | RPC `vault.source.set` `{name, enabled[, profile]}` | Disabling also locks the manager |

## Not run live (and why)

- `skills_hub_sources`, `skills_hub_search`, `skills_hub_preview`, `skills_hub_scan`,
  `skills_hub_install`, `skills_hub_uninstall`, `skills_hub_update` — network and/or spawn a
  CLI process.
- `toolset_models`, `toolset_select_model`, `toolset_select_provider` — entitlement/portal
  network calls or config writes; only the category-less `toolset_config` read was run.
- `toolset_save_env` (credentials), `toolset_post_setup` (spawns installers).
- `grant_computer_use_permissions` — spawns a process, macOS only.
- `mcp_catalog_install`, `mcp_servers_replace` — spawns/network and destructive whole-map write.
- `mcp_servers_set_api_key` (secret), `mcp_servers_test` (connects, spawns),
  `mcp_servers_oauth_start` / `poll` / `cancel` / `callback` (network + live OAuth session).
- `tools_configure` — writes config and may reset a live session; builder-tested only.
- `learning_node_update` / `learning_node_delete` / `learning_edit` / `learning_delete` —
  mutate user data.
- Every `vault.*` write (`vault_add`, `vault_remove`, `vault_unlock`, `vault_lock`,
  `vault_source_set`) — secrets or destructive. Only `vault.list` and `vault.sources` ran.
- `learning_node` did not run live because the isolated profile's graph had zero nodes
  (the test skips and prints `[SKIP]`); `learning.frames` did run.

## Not found / not implemented

- `/api/mcp/servers/file` — does **not** exist on the backend (the research note said so; the
  server-map editor uses `PUT /api/mcp/servers` instead).
- No REST route exists for `skills.reload`; the rescan is RPC-only.
- No route exposes computer-use permission state beyond `status`; there is no revoke.
- `mcp.servers.oauth.poll` responses contain action-specific extra keys; the typed struct
  keeps `ok`/`status` only and unknown keys are ignored by the lenient decoder.

## Verification

- Unit tests: `cargo test -p hermes-core --lib api::extras` — 23 passed (12 in `extras`, the
  rest in the parallel `extras::admin` submodule). Full `cargo test -p hermes-core`: 99 passed,
  0 failed.
- Live test (private backend on port 37273):
  `HERMES_DESKTOP_PORT=37273 cargo test -p hermes-core --test live_api_extras -- --ignored --nocapture`
  — **31 checks, 0 failed**.
