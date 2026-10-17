# API area: **system**

Typed client for the Hermes backend's system surface — status, logs, env keys, ops actions,
updates, gateway restart, usage analytics, setup, i18n, machine facts, agents/subagents/delegation,
spawn trees, wake word, diagnostics and `cli.exec`.

* Module: `crates/hermes-core/src/api/system.rs` (grouped by sub-feature, unit-tested)
* Live test: `crates/hermes-core/tests/live_api_system.rs` (private test backend only)
* Conventions: [`crate::api`](../../crates/hermes-core/src/api/mod.rs). `profile` is `""` for the
  default profile; REST uses `?profile=`, RPC sends `"profile": null`.

```sh
HERMES_DESKTOP_PORT=37273 cargo test -p hermes-core --test live_api_system -- --ignored --nocapture
```

42 endpoints: 15 REST + 27 JSON-RPC.

## REST

| Feature | Function | Backend call | Notes |
|---|---|---|---|
| Machine status | `system_status` / `system_status_request` | `GET /api/status` | Public probe: version, gateway state, sessions, auth gate, component rollup; loopback binds also carry `hermes_home`/`config_path`/`env_path`/`gateway_pid`/`gateways`. Read-only. |
| Log tail | `logs` / `logs_request` | `GET /api/logs?file&lines&level&component&search` | `file` ∈ agent, errors, gateway, gui, desktop, update, handoff, mcp; backend clamps `lines` (1–500, 2000 with `search`); 400 on unknown file/component. Read-only. |
| Env keys | `env_vars` / `env_request` | `GET /api/env` | Map name → `EnvVar` (redacted preview, provider grouping, `provider_profiles[]`). Read-only; the redacted sentinel is not a secret. |
| Reveal env value | `reveal_env` / `reveal_env_request` | `POST /api/env/reveal` `{key}` | **Reveals secrets**; backend rate-limits 5/30 s and audits. `RevealedEnvVar`'s `Debug` hides the value. Builder-only. |
| Doctor | `ops_doctor` / `ops_doctor_request` | `POST /api/ops/doctor` | **Spawns a process** in the background; poll `action_status("doctor")`. Builder-only. |
| Security audit | `ops_security_audit` / `ops_security_audit_request` | `POST /api/ops/security-audit` | **Spawns a process**; poll `action_status("security-audit")`. Builder-only. |
| Backup | `ops_backup` / `ops_backup_request` | `POST /api/ops/backup` `{output?}` | **Spawns a process**; `archive` in the reply names the generated zip. Builder-only. |
| Debug share | `ops_debug_share` / `ops_debug_share_request` | `POST /api/ops/debug-share` `{lines, redact}` | **Network** — uploads logs to paste services, returns `urls`/`failures`/`auto_delete_seconds`. Builder-only. |
| Action progress | `action_status` / `action_status_request` | `GET /api/actions/{name}/status?lines` | Log tail + liveness + optional `receipt` (free-form `Value`). 404 for unknown action names. Ran live with `doctor`. |
| Update check | `update_check` / `update_check_request` | `GET /api/hermes/update/check?force` | `install_method`, `behind`, `can_apply`, `commits[]`. **Network** (GitHub API). Builder-only. |
| Apply update | `update_apply` / `update_apply_request` | `POST /api/hermes/update` | **Spawns a process**; rewrites the install, may restart the gateway. Builder-only. |
| Gateway restart | `gateway_restart` / `gateway_restart_request` | `POST /api/gateway/restart` | **Spawns a process**; the live WebSocket drops. Builder-only. |
| Usage analytics | `usage_analytics` / `usage_analytics_request` | `GET /api/analytics/usage?days&profile` | `days` 1–365; `daily`/`by_model`/`by_task`/`totals` (nullable)/`skills`/`tools`. Read-only SQL. Ran live. |
| Local media | `media` / `media_request` | `GET /api/media?path` | Base64 `data_url` inside the backend's media roots; 400/403/404/413/415. Ran live as an error-path check only. |
| Remote media | `media_proxy` / `media_proxy_request` | `GET /api/media/proxy?url` | **Network** (allowlisted image CDNs). Builder-only. |

## JSON-RPC

| Feature | Function | Backend call | Notes |
|---|---|---|---|
| Config summary | `config_show` / `config_show_call` | `config.show {profile}` | Masked, display-ready `sections[].rows[][]`. Read-only. Ran live. |
| Setup (loose) | `setup_status` / `setup_status_call` | `setup.status {profile}` | `provider_configured`/`ready`/`free_tier_*`; all optional. Read-only. Ran live. |
| Setup (strict) | `setup_runtime_check` / `setup_runtime_check_call` | `setup.runtime_check {provider?, profile}` | Same resolver session creation uses. Live returned the documented retryable 5097 ("readiness check still in progress") on the provider-less test backend. |
| Languages | `i18n_languages` / `i18n_languages_call` | `i18n.languages {profile}` | Bundled ∪ overlay ∪ plugin packs, en first. Ran live (17 languages). |
| Catalog | `i18n_catalog` / `i18n_catalog_call` | `i18n.catalog {lang, surface, profile}` | `surface` ∈ core/tui/desktop; 4002 on a bad surface. Ran live. |
| Machine facts | `machine_facts` / `machine_facts_call` | `machine.facts {}` | `full_name` is stripped by the backend. Ran live. |
| Battery | `system_battery` / `system_battery_call` | `system.battery {profile}` | Always resolves (`available: false` without a battery). Ran live. |
| Capabilities | `gateway_capabilities` / `gateway_capabilities_call` | `gateway.capabilities {}` | `per_session_exclusive_submit`. Ran live. |
| Agents | `agents_list` / `agents_list_call` | `agents.list {profile}` | Registry-wide background `processes[]`. Read-only. Ran live. |
| Verification | `verification_status` / `verification_status_call` | `verification.status {session_id?, session_key?, cwd?, profile}` | Best known evidence; read-only, never runs checks. Ran live (`status=disabled`). |
| Insights | `insights_get` / `insights_get_call` | `insights.get {days?, profile}` | Session/message counts. Read-only. Ran live. |
| Usage bars | `usage_bars` / `usage_bars_call` | `usage.bars {profile}` | Two-bar dollar view; **Network** — fetches the Nous portal when logged in. Builder-only. |
| Delegation | `delegation_status` / `delegation_status_call` | `delegation.status {profile}` | Active tree + pause flag + limits. Read-only. Ran live. |
| Pause spawns | `delegation_pause` / `delegation_pause_call` | `delegation.pause {paused, profile}` | Reversible global write; not run live (affects the shared spawn gate). |
| Subagents | `subagent_list` / `subagent_list_call` | `subagent.list {session_id, profile}` | Live: 4001 for an unknown session (expected). |
| Subagent tail | `subagent_tail` / `subagent_tail_call` | `subagent.tail {session_id, subagent_id, profile}` | Live: 4001 for an unknown session (expected). |
| Subagent steer | `subagent_steer` / `subagent_steer_call` | `subagent.steer {session_id, subagent_id, text, profile}` | Write into a live child; builder-only. |
| Subagent interrupt | `subagent_interrupt` / `subagent_interrupt_call` | `subagent.interrupt {session_id, subagent_id, profile}` | **Destructive** to that child; builder-only. |
| Spawn tree save | `spawn_tree_save` / `spawn_tree_save_call` | `spawn_tree.save {subagents, session_id?, started_at?, finished_at?, label?, profile}` | Writes one JSON snapshot under the profile's spawn-trees dir (sub-agents free-form). Ran live with a `tag()` label, verified by list+load, then the file was removed and its absence re-verified. |
| Spawn tree list | `spawn_tree_list` / `spawn_tree_list_call` | `spawn_tree.list {session_id?, cross_session, limit?, profile}` | Read-only. Ran live. |
| Spawn tree load | `spawn_tree_load` / `spawn_tree_load_call` | `spawn_tree.load {path, profile}` | Path must be under the spawn-trees root (4030 otherwise). Ran live. |
| Handoff state | `handoff_state` / `handoff_state_call` | `handoff.state {session_id, profile}` | Live: 4001 for an unknown session (expected). |
| Handoff request | `handoff_request` / `handoff_request_call` | `handoff.request {session_id, platform, profile}` | **Network** — the gateway watcher posts to the platform's home channel. Builder-only. |
| Handoff fail | `handoff_fail` / `handoff_fail_call` | `handoff.fail {session_id, error?, profile}` | CAS write against the watcher; builder-only. |
| Wake word | `wake_status` / `wake_status_call` | `wake.status {surface?, client_capture?, profile}` | Read-only state (phrase, provider, input device, ownership). Ran live. |
| Diagnostics share | `diagnostics_share_nous` / `diagnostics_share_nous_call` | `diagnostics.share_nous {error_context?, extra_files?, log_lines?}` | **Network** — uploads a redacted bundle to Nous storage. Builder-only. |
| CLI exec | `cli_exec` / `cli_exec_call` | `cli.exec {argv, timeout?, profile}` | **Spawns a process** — runs `hermes <argv>`. Builder-only. |

## Not run live (and why)

| Function | Why |
|---|---|
| `reveal_env` | **Reveals a secret**; also rate-limited/audited. Verified by builder + unit tests only. |
| `ops_doctor`, `ops_security_audit`, `ops_backup` | Spawn background processes (brief rule). |
| `ops_debug_share` | Uploads logs to external paste services (network). |
| `update_check`, `update_apply` | Network (GitHub API) / spawns an install process. |
| `gateway_restart` | Spawns a process and drops the gateway the test backend depends on. |
| `media_proxy` | Fetches a remote URL (network). `media` was exercised live only on an error path (403), so its success shape is unit-tested only. |
| `usage_bars` | Fetches the Nous portal when logged in (network). |
| `delegation_pause` | Global write to the shared spawn gate; reversible but not exercised. |
| `subagent_steer`, `subagent_interrupt` | Act on live child agents; no session is owned by the test client. |
| `handoff_request`, `handoff_fail` | Post to a messaging platform / CAS-write a queue row. |
| `diagnostics_share_nous` | Uploads to Nous storage (network). |
| `cli_exec` | Runs arbitrary `hermes` subcommands (spawns a process). |

Also: the success body of `setup.runtime_check` was not observed on the private test backend
(it answers the documented retryable 5097); the retryable code itself is what the live check
verifies. `spawn_tree.*` ran live and the throwaway snapshot was deleted afterwards.

## Not found / not implemented

None in scope: every endpoint listed in the area research exists in the backend
(`hermes_cli/web_routers/*` for REST, `tui_gateway/methods_*.py` + the OpenRPC contract for RPC).
Out of area and deliberately not included (they belong to other area modules): cron basics,
memory/curator, skills/toolsets/MCP, webhooks, pairing, credentials pool, profiles, config
read/write and `/api/system/stats`.
