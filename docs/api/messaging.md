# Messaging, pairing and webhooks

Typed wrappers for the dashboard's messaging surface: platform cards, Telegram and
WhatsApp QR onboarding, DM pairing, and webhook subscriptions.

* Module: `hermes_core::api::messaging` (`crates/hermes-core/src/api/messaging.rs`)
* Backend: `hermes_cli/web_routers/messaging.py` (platforms + onboarding) and
  `hermes_cli/web_routers/ops.py` (pairing + webhooks)
* Electron counterpart: `apps/desktop/src/api/messaging.ts`
* Coverage: **20 REST routes, 21 wrapper functions** (approve has two bodies on one
  route). No gateway RPC method is involved — this area is REST-only.

Every endpoint follows the module conventions: a pure builder (`*_request`) that unit
tests pin to the exact method/path/query/body, an async wrapper that decodes a lenient
response struct, and `#[serde(default)]` on every reply type so a field the backend
adds or omits later never breaks the client.

Profile scope: the routes take `?profile=` (`profile_query`). `POST /api/pairing/approve`
and `POST /api/pairing/revoke` read the profile from the **body**, so the builders put it
there (body wins over the query on the backend). `DELETE /api/webhooks/{name}` is the one
route behind the backend's destructive-profile guard, so it always names the profile
explicitly (`explicit_profile_query`, `?profile=default` when the caller means default).

## Platforms

| Feature | Function | Backend call | Notes |
|---|---|---|---|
| Platform cards | `messaging_platforms` (`messaging_platforms_request`) | `GET /api/messaging/platforms?profile=` | Read-only: 32 catalog cards with state, `env_vars`, ingress URL. Verified live. |
| Configure platform | `messaging_platform_update` (`messaging_platform_update_request`) | `PUT /api/messaging/platforms/{id}?profile=` | **Writes credentials** (`env`, `clear_env`) and/or the `enabled` flag; `profile` is also sent in the body. Live: empty-body no-op verified (writes nothing). |
| Test platform | `messaging_platform_test` (`messaging_platform_test_request`) | `POST /api/messaging/platforms/{id}/test?profile=` | **Network** — probes the live gateway for the connection verdict. Not run live. |

## Telegram QR onboarding

| Feature | Function | Backend call | Notes |
|---|---|---|---|
| Start pairing | `telegram_onboarding_start` (`…_request`) | `POST /api/messaging/telegram/onboarding/start?profile=` | **Network** — mints a bot through the Nous pairing service; returns `pairing_id`, `deep_link`, `qr_payload`. Not run live. |
| Poll pairing | `telegram_onboarding_status` (`…_request`) | `GET /api/messaging/telegram/onboarding/{id}?profile=` | **Network** — the backend asks the pairing service for the state. Live: unknown id → 404 (local lookup). |
| Apply pairing | `telegram_onboarding_apply` (`…_request`) | `POST /api/messaging/telegram/onboarding/{id}/apply?profile=` | **Network**, **writes credentials** (bot token + allowlist), **spawns a best-effort gateway restart**. Not run live. |
| Cancel pairing | `telegram_onboarding_cancel` (`…_request`) | `DELETE /api/messaging/telegram/onboarding/{id}?profile=` | Forgets the in-flight pairing locally; idempotent, the pairing service is not told. Live: unknown id → `{"ok": true}`. |

## WhatsApp QR onboarding

| Feature | Function | Backend call | Notes |
|---|---|---|---|
| Start pairing | `whatsapp_onboarding_start` (`…_request`) | `POST /api/messaging/whatsapp/onboarding/start?profile=` | **Network** and **spawns a process** — installs the bridge and runs the pairing child. Returns the QR payload. Not run live. |
| Poll pairing | `whatsapp_onboarding_status` (`…_request`) | `GET /api/messaging/whatsapp/onboarding/{id}?profile=` | Local session state (410 once expired). Live: unknown id → 404. |
| Apply pairing | `whatsapp_onboarding_apply` (`…_request`) | `POST /api/messaging/whatsapp/onboarding/{id}/apply?profile=` | **Writes credentials** (mode, allowlist, linked account), **spawns a gateway restart**. Not run live. |
| Cancel pairing | `whatsapp_onboarding_cancel` (`…_request`) | `DELETE /api/messaging/whatsapp/onboarding/{id}?profile=` | **Terminates the pairing child**; idempotent. Live: unknown id → `{"ok": true}`. |

## Pairing (who may DM the bot)

| Feature | Function | Backend call | Notes |
|---|---|---|---|
| Pairing state | `pairing` (`pairing_request`) | `GET /api/pairing?profile=` | Read-only: `pending` requests and `approved` users. Verified live (default and `swe`). |
| Approve by request id | `pairing_approve` (`pairing_approve_request`) | `POST /api/pairing/approve` | Body `{platform, request_id, profile}` — **grants access** to one user. Not run live. |
| Approve by code | `pairing_approve_code` (`pairing_approve_code_request`) | `POST /api/pairing/approve` | Body `{platform, code, profile}`; the backend upper-cases the code and rate-limits misses (429). **Grants access.** Not run live. |
| Revoke | `pairing_revoke` (`…_request`) | `POST /api/pairing/revoke` | Body `{platform, user_id, profile}`. **Destructive** — the user loses access. Not run live. |
| Clear pending | `pairing_clear_pending` (`…_request`) | `POST /api/pairing/clear-pending?profile=` | **Destructive** — drops every pending request (published codes stop working). Backend route; the Electron client does not call it. Not run live. |

## Webhooks

| Feature | Function | Backend call | Notes |
|---|---|---|---|
| List subscriptions | `webhooks` (`webhooks_request`) | `GET /api/webhooks?profile=` | Read-only: `enabled`, `base_url`, every route (`secret_set`, never the secret). Verified live. |
| Enable platform | `webhooks_enable` (`…_request`) | `POST /api/webhooks/enable?profile=` | **Spawns a process** — writes the config flag, then a best-effort gateway restart. Not run live. |
| Create subscription | `webhook_create` (`webhook_create_request`) | `POST /api/webhooks?profile=` | **Stores an HMAC secret** returned exactly once (`WebhookCreated.secret`); 400 while the platform is disabled. Live: the 400 refusal was verified, the round trip was not run (see below). |
| Toggle subscription | `webhook_set_enabled` (`…_request`) | `PUT /api/webhooks/{name}/enabled?profile=` | Enables/disables in place; a disabled route's events are rejected with 403. Not run live (no route exists on the test backend). |
| Delete subscription | `webhook_delete` (`…_request`) | `DELETE /api/webhooks/{name}?profile=` | **Destructive**; the only route here behind the destructive-profile guard, so the profile is always explicit. Not run live. |

`WebhookCreate` (`name`, optional `description`/`events`/`prompt`/`script`/`skills`/
`deliver`/`deliver_chat_id`/`secret`) is the only serialised request body; every other
body is built field-by-field and omits untouched keys.

## Verification

Unit tests: 11 tests in the module (every builder's method/path/query/body, every reply
type decoded from a handler-shaped fixture plus an empty `{}`, and one negative decode);
87 tests pass crate-wide. `cargo clippy -p hermes-core --all-targets -- -D warnings`
is clean.

Live: `HERMES_DESKTOP_PORT=37273 cargo test -p hermes-core --test
live_api_messaging -- --ignored --nocapture` → **17 checks, 0 failed** against the
private test backend. Exercised: platform list (default + `swe`), the empty-body
platform update (a true no-op write), pairing state (default + `swe`), webhook list, the
webhook-create refusal while the platform is disabled, and the two onboarding status /
cancel calls for an unknown pairing id.

## Not run live (and why)

* **Platform configure / test** — writing credentials is irreversible, and `/test`
  contacts the live platform gateway.
* **Telegram onboarding start / apply** — mints a bot on the Nous pairing service and
  writes its token into the profile; apply also restarts the gateway.
* **WhatsApp onboarding start / apply** — installs the bridge (network + child process)
  and apply restarts the gateway.
* **Pairing approve / revoke / clear-pending** — they grant or remove access, and
  clear-pending would delete rows created outside this run. The test backend's store is
  empty, so there is nothing safe to approve or revoke.
* **`POST /api/webhooks/enable`** — restarts the gateway (spawns a process).
* **Webhook create → toggle → delete round trip** — the test backend's webhook platform
  is disabled, and enabling it requires the route above. Only the 400 "not enabled"
  refusal was checked live; the wrappers for create/toggle/delete are builder- and
  unit-tested only. On a backend with webhooks enabled, the round trip would create
  `api-test-<pid>`, verify it in a fresh list, disable/re-enable it, delete it and verify
  it is gone.

## Not found / not implemented

* No `GET /api/webhooks/{name}` (single-route read) and no `/api/webhooks/my`: the store
  is only readable as a whole via `GET /api/webhooks`.
* No gateway RPC methods for this area — the backend routers and
  `apps/desktop/src/api/messaging.ts` are REST-only.
* No per-platform delete/reset route: a platform is cleared with
  `clear_env` + `enabled: false` through the update endpoint.
* No webhook update-in-place beyond the `enabled` toggle; changing events, prompt, script
  or delivery means re-creating the route (create replaces by name).
* No per-platform pairing list; `GET /api/pairing` returns all platforms together.
* Telegram onboarding cancel is local-only (nothing is sent to the pairing service), and
  a cancelled WhatsApp session cannot be applied later.
