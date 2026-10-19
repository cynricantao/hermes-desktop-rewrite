# Models, providers and local models — `api::models`

Typed client for the Models / Providers / Local Models pages. Code:
`crates/hermes-core/src/api/models.rs`. Live checks: `crates/hermes-core/tests/live_api_models.rs`
(`HERMES_DESKTOP_PORT=<private test port> cargo test -p hermes-core --test live_api_models -- --ignored`).

Every route takes `profile: &str` (`""` = default) and scopes the backend config the same way
`/api/model/set` writes it. `ModelOptionsResult` / `ModelOptionProvider` are re-exported from
`crate::protocol`, so the Models page and the model picker share one type.

## Model assignment — `/api/model/*`

| Feature | Function | Backend call | Live |
|---|---|---|---|
| Resolved context length + capabilities | `model_info` / `model_info_request` | `GET /api/model/info` (5 s probe budget, `MODEL_INFO_TIMEOUT`) | verified |
| Picker inventory | `model_options` / `model_options_request` | `GET /api/model/options?refresh&include_unconfigured&explicit_only` | verified |
| Same over the gateway | `model_options_rpc` / `model_options_call` | RPC `model.options` | verified |
| Assign main / auxiliary model | `model_set` / `model_set_request` | `POST /api/model/set` | builder-only — writes `config.yaml` |
| Auxiliary slots + main | `auxiliary_models` / `auxiliary_models_request` | `GET /api/model/auxiliary` | verified |
| MoA config | `moa` / `moa_request` | `GET /api/model/moa` | verified |
| Save MoA config | `moa_set` / `moa_set_request` | `PUT /api/model/moa` (note: **PUT**, not POST) | builder-only — writes `config.yaml` |
| Onboarding default | `recommended_default` / `recommended_default_request` | `GET /api/model/recommended-default?provider=` | verified |

`ModelAssignment` carries the backend's three-way `reasoning_effort`: unset key = leave alone,
`Some(None)` = clear, `Some(Some(level))` = set. A reply with `ok: false` +
`confirm_required: true` is the expensive-model guard: nothing was written; show
`confirm_message` and repeat with `.confirm_expensive()`.

## Providers — `/api/providers/*`

| Feature | Function | Backend call | Live |
|---|---|---|---|
| OAuth/external provider cards | `oauth_providers` | `GET /api/providers/oauth` | verified |
| Start device-code login | `oauth_start` | `POST /api/providers/oauth/{id}/start` | builder-only — network, saves tokens |
| Poll login | `oauth_poll` | `GET /api/providers/oauth/{id}/poll/{session}` | builder-only (needs a live session) |
| Cancel login | `oauth_cancel` | `DELETE /api/providers/oauth/sessions/{id}` | builder-only — kills the poller |
| Submit PKCE code | `oauth_submit` | `POST /api/providers/oauth/{id}/submit` | builder-only; always 400 today |
| Disconnect provider | `oauth_disconnect` | `DELETE /api/providers/oauth/{id}` | builder-only — deletes credentials |
| Validate a credential | `provider_validate` | `POST /api/providers/validate` | builder-only — sends the key to the provider |
| Custom endpoints list | `custom_endpoints` | `GET /api/providers/custom-endpoints` | verified |
| Create/update endpoint | `custom_endpoint_upsert` | `POST /api/providers/custom-endpoints` | builder-only — config + `.env` |
| Make it the main model | `custom_endpoint_activate` | `POST /api/providers/custom-endpoints/{id}/activate` | builder-only — config |
| Delete endpoint | `custom_endpoint_delete` | `DELETE /api/providers/custom-endpoints/{id}` | builder-only — destructive |
| Probe an endpoint | `custom_endpoint_validate` | `POST /api/providers/custom-endpoints/validate` | builder-only — contacts the URL with the key |

`CredentialValidation` / `CustomEndpointValidation` distinguish **bad key** (`ok: false,
reachable: true` — block the save) from **probe could not run** (`reachable: false` — save with a
warning). `CustomEndpointUpdate.api_key` is tri-state: `None` keeps the stored key, `Some("")`
clears it, `Some(key)` writes it.

## Local models — `/api/local-models/*`

| Feature | Function | Backend call | Live |
|---|---|---|---|
| Runtime + staged models | `local_models_status` | `GET /api/local-models/status` | builder-only (not in the live set; local disk scan) |
| GPU/RAM budget | `local_hardware` | `GET /api/local-models/hardware` | builder-only (runs `nvidia-smi`) |
| Priced catalog | `local_catalog` | `GET /api/local-models/catalog` | builder-only — kicks a **background network refresh** |
| Job list / poll | `local_jobs`, `local_job` | `GET /api/local-models/jobs[/{id}]` | verified (list + 404 path) |
| Install engine | `runtime_install` | `POST /api/local-models/runtime/install` | builder-only — download, unpack, spawn |
| Download a model | `local_download` | `POST /api/local-models/download` | builder-only — Hugging Face download |
| Pause / resume | `local_download_pause`, `local_download_resume` | `POST .../download/{pause,resume}` | builder-only — job control |
| Delete a model | `local_model_delete` | `DELETE /api/local-models/models/{id}` | builder-only — destructive |
| One-click setup | `quickstart` | `POST /api/local-models/quickstart` | builder-only — runtime + weights + config |
| Server on/off | `local_server` | `POST /api/local-models/server` | builder-only — process control, frees GPU |
| Eject from GPU | `local_eject` | `POST /api/local-models/eject` | builder-only — GPU memory |
| Make default | `local_activate` | `POST /api/local-models/activate` | builder-only — config + server |
| HF search / repo files | `local_search`, `local_search_files` | `GET .../search`, `.../search/files` | builder-only — huggingface.co |
| Download browsed files | `download_browsed` | `POST /api/local-models/download-browsed` | builder-only — network |
| Sideload a local GGUF | `sideload` | `POST /api/local-models/sideload` | builder-only — writes the models dir |

`LocalJob` mirrors `_job_view`: `status` is `running | paused | done | error`, `percent` appears
once `total_bytes` is known, `bytes_per_sec` / `eta_seconds` only while running,
`can_pause` / `can_resume` are the backend's own answers (do not re-derive them).

## RPCs

| Feature | Function | Call | Live |
|---|---|---|---|
| Disconnect a provider | `model_disconnect` | `model.disconnect {slug, session_id?}` → `{slug, name, disconnected}` | builder-only — destroys credentials |
| Save a provider key | `model_save_key` | `model.save_key {slug, api_key, session_id?}` → `{provider}` | builder-only — writes a secret |

Both are `_profile_scoped` on the gateway: pass the `session_id` of the chat so the write lands
in that session's profile. `model.disconnect` answers error 4005 when nothing was stored;
`model.save_key` refuses a managed install (4006) and non-`api_key` auth types (4003).
