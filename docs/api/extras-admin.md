# API extras — admin (cron, profiles, memory providers)

Typed clients for `crate::api::extras::admin`: cron blueprints / delivery targets / job
read-update / `cron.manage`, profile extras (active, description, model, describe-auto,
export/import, setup-command, open-terminal, desktop overlay, `profiles.*` RPCs, avatar
assets) and memory provider config / setup / OAuth.

Conventions, helpers and the response-type rules live in [`crate::api`]. Profile
create/rename/delete/soul and the cron create/pause/resume/trigger/delete REST routes are
**not** here — they already exist in [`crate::capabilities`].

Every *Builder* is a pure function (`*_request` → `Request`, `*_call` → `Call`) and is
unit-tested for method, path, query, body and response decoding; each *Function* is the async
wrapper that sends it.

## Endpoint map

| Feature | Function | Backend call | Notes |
|---|---|---|---|
| Cron — blueprint catalog | `cron_blueprints` | `GET /api/cron/blueprints?profile=` | Line 1 of the "New job" form. Live (16 blueprints on the test backend). |
| Cron — blueprint instantiate | `cron_blueprint_instantiate` | `POST /api/cron/blueprints/instantiate?profile=default` | Body `{blueprint, values}`; returns the created job. **Creates a scheduled job** — builder-only (unit-tested). |
| Cron — delivery targets | `cron_delivery_targets` | `GET /api/cron/delivery-targets?profile=` | Implicit `local` + each gateway platform (`home_target_set:false` when no home channel). Live (4 targets). |
| Cron — job read | `cron_job` | `GET /api/cron/jobs/{id}?profile=` | Normalized record (`id`/`name`/`prompt`/`schedule_display` always strings). Live on a throwaway job. |
| Cron — job update | `cron_job_update` | `PUT /api/cron/jobs/{id}?profile=` | Body `{updates:{…}}`; pass only changed fields. Live on a throwaway job, then deleted (404 verified). |
| Cron — manage list | `cron_manage_list` | RPC `cron.manage` `{action:"list"}` | Tool view (id under `job_id`, `prompt_preview`, `schedule` display). Live: sees the REST-created job. |
| Cron — manage add | `cron_manage_add` | RPC `cron.manage` `{action:"add"}` | `deliver`/`repeat`/`continuity` optional. **Creates a scheduled job** — builder-only. |
| Cron — manage pause/resume/remove | `cron_manage_job` | RPC `cron.manage` `{action:"pause"\|"resume"\|"remove", name}` | **Destructive** for remove — builder-only. |
| Profile — set active | `profile_active_set` | `POST /api/profiles/active` | Sticky CLI default (`hermes profile use`). **Changes future CLI/gateway defaults** — builder-only. |
| Profile — set description | `profile_description_set` | `PUT /api/profiles/{name}/description` | Marks it user-authored (`description_auto:false`). Live on a throwaway profile. |
| Profile — set model | `profile_model_set` | `PUT /api/profiles/{name}/model` | Validated against the provider catalog: a bogus pick answers a clean 400. Live (400 observed & accepted). |
| Profile — describe-auto | `profile_describe_auto` | `POST /api/profiles/{name}/describe-auto` | **Spends model calls** (aux-LLM round trip; failure is `ok:false`+`reason`, not HTTP). Builder-only. |
| Profile — export | `profile_export` | `POST /api/profiles/{name}/export` | Body `{output, extra_files}` → `{ok, archive}`; empty `output` = backend export dir. **Writes an archive** — builder-only. |
| Profile — import | `profile_import` | `POST /api/profiles/import` | Body `{archive, name?}` → `{ok, name, path, desktop}`. **Creates a profile** — builder-only. |
| Profile — setup command | `profile_setup_command` | `GET /api/profiles/{name}/setup-command` | Live (command echoes the profile name). |
| Profile — open terminal | `profile_open_terminal` | `POST /api/profiles/{name}/open-terminal` | **Spawns a terminal emulator** on the backend host — builder-only. |
| Profile — desktop overlay | `profile_desktop_overlay` | `GET /api/profiles/{name}/desktop-overlay` | `{exists, desktop}`; `desktop` null when absent. Live on a fresh throwaway profile (`exists:false`). |
| Profile — roster | `profiles_list` | RPC `profiles.list` | Rows + `bot_mode_protocol` + `install_id`; `include_sessions` adds last/worker/canonical session rows. Live (create → listed → delete → gone). |
| Profile — editor snapshot | `profile_describe` | RPC `profiles.describe` | `{name, description, soul, model{provider,default}, skills[], toolsets[], toolsets_pinned, mcp_servers[]}`. Live. |
| Profile — configure | `profile_configure` | RPC `profiles.configure` | `patch` = any of `ui_meta`(+expected revisions), `soul`, `description`, `model`+`provider`, `disabled_skills`, `enabled_toolsets`, `enabled_mcp_servers`; `applied` reports each section. **May switch models** (echoes `confirm_required`). Live on a throwaway profile. |
| Profile — avatar write | `profiles_set_asset` | RPC `profiles.set_asset` | `asset:"avatar"`, data URL or bare base64 (PNG/JPEG/WebP ≤2 MB, sniffed), or `clear:true`. **Writes a file.** Live (set 1×1 PNG on throwaway). |
| Profile — avatar read | `profiles_get_asset` | RPC `profiles.get_asset` | Data URL; `found:false` is not an error. Live (found, mime `image/png`). |
| Profile — create (RPC) | `profiles_create` / `profiles_create_call` | RPC `profiles.create {name, …}` | JSON-RPC twin of `POST /api/profiles`; `ProfileCreateOptions` = description, clone_from/all/channels, no_skills, soul, model+provider, share_auth, no_alias; `mirror_credentials` default true. **Writes a profile directory + alias script** — builder-only (the REST create in `capabilities` holds the live coverage). |
| Memory — config form | `memory_provider_config` | `GET /api/memory/providers/{name}/config?surface=&profile=` | `surface=declared` = manifest schema the desktop panel renders. Secrets are write-only (`value:""`, `is_set`). Live (2/2 providers decoded; values never printed). |
| Memory — config save | `memory_provider_config_update` | `PUT /api/memory/providers/{name}/config` | Body `{values}`; secrets go to the profile `.env`; declared surface sets the provider active. **Writes config/secrets** — builder-only. |
| Memory — setup | `memory_provider_setup` | `POST /api/memory/providers/{name}/setup` | Optional `{values}`; installs pip/external deps and runs setup commands. **Spawns processes** — builder-only. |
| Memory — OAuth start | `memory_provider_oauth_start` | `POST /api/memory/providers/{provider}/oauth/start` | Zero-CLI browser flow; payload is flow-defined (returned as `Value`). **Network + opens a browser** — builder-only. |
| Memory — OAuth status | `memory_provider_oauth_status` | `GET /api/memory/providers/{provider}/oauth/status` | `idle \| pending \| connected \| error` — builder-only (meaningful only after a start). |

### Response types

`CronBlueprint`/`CronBlueprints`, `CronBlueprintField` (wire `type` → `field_type`; camelCase
`scheduleHuman`/`appUrl` kept via rename), `CronDeliveryTarget(s)`, `CronJob` (REST record),
`CronManageJob` (tool view; use `id()` — list rows carry `job_id`, the `removed_job` row
carries `id`), `CronManageResult` (also carries the **tool-level** `error`/`success` that the
cron tool returns inside a successful RPC), `ProfileActiveSet`, `ProfileDescriptionSet`,
`ProfileModelSet`, `ProfileDescribed`, `ProfileExported`, `ProfileImported`,
`ProfileSetupCommand`, `ProfileTerminalOpened`, `ProfileDesktopOverlay`, `ProfileRow`/
`ProfileList` (`model`/`provider` are `Option<String>` — unpinned profiles send `null`),
`ProfileSnapshot` (+ `ProfileModel`, `ProfileSkill`, `ProfileToolset`, `ProfileMcpServer`),
`ProfileConfigured`, `ProfileAssetSet`, `ProfileAssetData`, `MemoryProviderConfig`,
`MemoryProviderField`, `MemoryFieldOption`, `MemoryProviderConfigUpdated`,
`MemorySetupStep`, `MemorySetupResult`, `MemoryOAuthStatus`.

## Live verification

```
HERMES_DESKTOP_PORT=37273 cargo test -p hermes-core --test live_api_extras_admin -- --ignored --nocapture
```

`crates/hermes-core/tests/live_api_extras_admin.rs` — 22 checks, 0 failed. Reads plus two
throwaway items created with `capabilities` builders and deleted with verified 404/absence:
a cron job (create → get → update → `cron.manage` list → delete → 404) and a profile
(create → setup-command → desktop-overlay → description → model 400 → configure → list →
describe → avatar set/get → delete → absent). Observed on the test backend: 16 blueprints,
4 delivery targets, 2 memory providers (declared config decodes; no fields declared for
either), and `PUT .../model` with a bogus model answers `400 Model … was not found in this
provider's model listing.`

## Not run live (and why)

* `cron_blueprint_instantiate`, `cron_manage_add` — create real scheduled jobs (a delete-only
  run was enough for this pass; both are builder + unit-tested).
* `profile_active_set` — would change the test backend's sticky CLI default.
* `profile_describe_auto` — spends model calls.
* `profile_export` / `profile_import` — write archives / create profiles from files the test
  does not own.
* `profile_open_terminal` — spawns a terminal emulator (no desktop session on the test host).
* `profiles_create` (RPC) — creates a real profile directory and alias script; builder +
  unit-tested here, while the live-tested create path stays the REST one in `capabilities`.
* `memory_provider_config_update` — writes config and secrets; `memory_provider_setup` —
  installs packages; both OAuth wrappers — network/browser.
* `cron_manage_job` (pause/resume/remove) — the REST routes for these are already live-tested
  through `capabilities`; the RPC twin stays unit-tested.

## Not found / not implemented

* Profile create / rename / delete / soul and cron create / pause / resume / trigger / delete
  — covered by `capabilities` (REST). The `profiles.create` RPC twin is implemented here for
  API-surface parity (builder + unit tests); the REST path keeps the live coverage.
* `GET /api/cron/jobs/{id}/runs` (run history: `cron_job_runs`, ran live with an unknown id)
  and `POST /api/cron/jobs/{id}/trigger` (`cron_job_trigger`, **runs the job now: spends
  model calls**, builder-only) are wrapped. `POST /api/cron/fire` (NAS gateway webhook) is
  out of scope.
* No `GET /api/profiles/{name}/description` (only PUT); no separate avatar route (use
  `set_asset` with `clear`).
* Memory providers have no list route of their own (`GET /api/memory` lists them —
  `capabilities`); memory OAuth has only start/status (the `submit`/`cancel`/`poll` routes
  exist for model providers, not memory providers).
