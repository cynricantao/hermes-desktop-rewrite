# Billing, subscription & free tier (JSON-RPC)

`crates/hermes-core/src/api/billing.rs`. All methods are JSON-RPC on the gateway WebSocket;
handlers live in `tui_gateway/methods_session.py` (billing / subscription),
`tui_gateway/methods_free_tier.py` and `tui_gateway/methods_onboarding.py`.

## Safety first

Billing replies are **structured envelopes**: the gateway answers `result.ok` / `result.error`
rather than a JSON-RPC error, so a wrapper returns `Ok(..)` even when the portal refused. Check
`ok`, then branch on `error` (`insufficient_scope` → run `billing_step_up`, `payment_failed`,
`rate_limited`, `session_revoked`, `remote_spending_revoked`, …). Refusals may also carry
`portal_url`, `retry_after`, `actor`, `code`, `recovery`.

Money is sent and received as **strings** (`"12.34"`) to keep decimals exact; `*_display` fields
are pre-formatted labels a UI can render unchanged.

| # | Rule |
|---|---|
| 1 | **Nothing in this area is called by a live test except `free_tier.status`** — every other method touches the Nous portal, moves money, changes the plan or writes config. |
| 2 | `billing.charge` / `subscription.upgrade` take an idempotency key (`new_idempotency_key()`); **reuse it on retry**, or a timed-out retry can charge twice. |
| 3 | `billing.charge` is only the *start*: poll `billing.charge_status` until `settled` / `failed`. Never report success from the charge reply alone. |
| 4 | `billing.step_up` blocks for minutes (device flow) — use `BILLING_STEP_UP_TIMEOUT` (300 s) and run it off the UI thread; the URL/code arrive via the `billing.step_up.verification` notification. |
| 5 | Confirm with the user before any wrapper marked **Charges money** / **changes the plan**. |

## Methods

| Feature | Function (builder / wrapper) | Backend call | Live? |
|---|---|---|---|
| Billing view | `billing_state_call` / `billing_state` → `BillingState` | `billing.state` (portal) | builder-only — portal round-trip |
| Auto top-up setting | `billing_auto_reload_call` / `billing_auto_reload` → `BillingMutation` | `billing.auto_reload` (portal write) | builder-only — **charges money** |
| Start a top-up | `billing_charge_call` / `billing_charge` → `BillingCharge` | `billing.charge` (portal write) | builder-only — **charges money** |
| Charge status | `billing_charge_status_call` / `billing_charge_status` → `BillingChargeStatus` | `billing.charge_status` (portal) | builder-only — portal round-trip |
| Unlock billing:manage | `billing_step_up_call` / `billing_step_up` → `BillingStepUp` | `billing.step_up` (device flow) | builder-only — network, blocks minutes |
| Subscription view | `subscription_state_call` / `subscription_state` → `SubscriptionState` | `subscription.state` (portal) | builder-only — portal round-trip |
| Quote a plan change | `subscription_preview_call` / `subscription_preview` → `SubscriptionPreview` | `subscription.preview` (portal) | builder-only — portal round-trip |
| Schedule downgrade / cancel | `subscription_change_call` / `subscription_change` → `SubscriptionPendingChange` | `subscription.change` (portal write) | builder-only — **changes the plan** |
| Keep the plan renewing | `subscription_resume_call` / `subscription_resume` → `SubscriptionPendingChange` | `subscription.resume` (portal write) | builder-only — **changes the plan**, re-enables recurring spend |
| Upgrade now | `subscription_upgrade_call` / `subscription_upgrade` → `SubscriptionUpgrade` | `subscription.upgrade` (portal write) | builder-only — **charges money and changes the plan** |
| Free-tier status | `free_tier_status_call` / `free_tier_status` → `FreeTierStatus` | `free_tier.status` (local) | **live-verified** — local auth-store read, no side effects |
| Retry trial claim | `free_tier_provision_call` / `free_tier_provision` → `FreeTierProvision` | `free_tier.provision` (portal) | builder-only — **network**, writes the identity |
| Claim sign-in nudge | `free_tier_claim_nudge_call` / `free_tier_claim_nudge` → `FreeTierClaimNudge` | `free_tier.claim_nudge` (local write) | builder-only — one-shot write |
| Report challenge outcome | `free_tier_challenge_result_call` / `free_tier_challenge_result` → `FreeTierChallengeResult` | `free_tier.challenge_result` (local write) | builder-only — one-shot write |
| Acknowledge notice | `free_tier_ack_notice_call` / `free_tier_ack_notice` → `FreeTierAck` | `free_tier.ack_notice` (local write) | builder-only — one-shot write |
| Onboarding run flag | `onboarding_set_run_call` / `onboarding_set_run` → `OnboardingState` | `onboarding.set_run` (root config write) | builder-only — writes config |

`billing.state`, `subscription.state`, `billing.auto_reload` and the free-tier methods accept
`profile` (`""` = the launch profile). `onboarding.set_run` is unscoped on purpose: it writes the
root profile's `config.yaml` whichever profile the backend serves.

## Types worth knowing

* `BillingState` — `free_tier_account` marks the Nous free tier (no account, no balance): branch on
  it **before** `logged_in`. `usage` is the shared two-bar model (`api::system::UsageBars`).
* `BillingChargeStatus.amount_usd` is `BillingScalar` (string **or** number).
* `SubscriptionPreview.effect` — `charge_now` | `scheduled` | `no_op` | `blocked`; show it before
  confirming an upgrade.
* `SubscriptionUpgrade.status` — only `ok` means the plan changed; `requires_action` /
  `payment_failed` needs the portal at `recovery_url`.
* `FreeTierStatus.challenge` — the same payload the `free_tier.challenge` notification carried, for
  a client that connected after it fired.

## Notifications

| Event | Constant | Parser | Payload |
|---|---|---|---|
| `billing.step_up.verification` | `BILLING_STEP_UP_VERIFICATION_EVENT` | `parse_billing_step_up_verification` | `BillingStepUpVerification { verification_url, user_code }` — sent on the `session_id` passed to `billing_step_up`; the client opens the browser |
| `free_tier.challenge` | `FREE_TIER_CHALLENGE_EVENT` | `parse_free_tier_challenge` | `FreeTierChallenge { type: "browser", url, required, expires_in, message, attempt }` — broadcast, no session; answer with `free_tier_challenge_result` echoing `attempt` |

Both parsers take `(event_type, payload)` and return `None` for any other event, so a UI can feed
them every `GatewayEvent`.

## Tests

* Unit: `cargo test -p hermes-core --lib api::billing` (11 tests — every builder's method/params,
  realistic replies from the serializers, `{}` for every response type, notification parsing).
* Live: `cargo test -p hermes-core --test live_api_billing -- --ignored` against
  `scripts/live-test-backend.sh` — reachability plus `free_tier.status` only.
