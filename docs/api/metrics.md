# Metrics API (`api::metrics`)

Typed client for the backend's shared-metrics area: telemetry consent and the Desktop's usage
reports. Every endpoint is a JSON-RPC method over the gateway WebSocket; there is no REST half.
UI-free: the controller turns these into the consent strip / dialog and the fire-and-forget
report calls.

Sources of truth: `tui_gateway/methods_shared_metrics.py` (handlers, params, reply shape),
`hermes_cli/observability/shared_metrics_{consent,events,desktop,startup,update}.py` (gating),
`apps/shared/src/gateway-contract.openrpc.json` (`SharedMetrics*` schemas), and the Electron
call sites listed per row.

All ten methods are `@_profile_scoped`: every builder takes `profile` (`""` = the connection's
focused profile) and adds `"profile"` to the params only when it names one. A backend older
than the method answers a JSON-RPC error, which surfaces as `ApiError::Rpc` — the Electron
settings screen disables the controls and explains instead.

| Feature | Function | Backend call | Notes |
|---|---|---|---|
| Consent state | `status_call` / `status` | `shared_metrics.status` `{profile?}` | **Read-only** pure read of the profile's `config.yaml` (no defaults merged). Reply `{enabled, send, decided, reask}`. Error 5095 when the config cannot be read. Electron: `store/shared-metrics.ts` `readSharedMetricsConsent` (treats a failure as `null` = don't ask). |
| Set consent | `set_call` / `set` | `shared_metrics.set` `{enabled, send, first_run, profile?}` | **Changes the user's telemetry consent.** `send` is ignored unless `enabled` (the wizard's rule); turning collection off also withdraws send consent, purges onboarding latches and drops pending upload receipts. `first_run: true` marks the first-run answer and records `setup_completed`. Reply is the stored consent. Error 5096 on write failure. Electron: `saveSharedMetricsConsent`, `answerSharedMetricsOffer`. |
| Slash command used | `slash_command_call` / `slash_command` | `shared_metrics.slash_command` `{command, session_id?, profile?}` | **Telemetry.** Once per user-typed command, locally handled ones included; `command` is the bare name (no `/`, no args). `session_id` scopes the count to that session's profile. Always `{ok: true}`. Electron: `use-prompt-actions/slash.ts`. |
| Startup latency | `startup_latency_call` / `startup_latency` | `shared_metrics.startup_latency` `{elapsed_ms, surface, launch_id, profile?}` | **Telemetry.** Client-measured launch → ready, once per launch; `surface` is `StartupSurface::{DesktopAttach,Tui}` (`desktop_attach` \| `tui`, declared because a URL/cloud backend has no `HERMES_DESKTOP` to detect); the backend latches on the opaque `launch_id` so a reconnect does not re-count. Always `{ok: true}`. Electron: `app/gateway/report-startup-latency.ts`. |
| Self-update run | `update_run_call` / `update_run` | `shared_metrics.update_run` `{outcome, failed_stage?, mechanism?, duration_ms?, from_commit_date?, profile?}` | **Telemetry.** One packaged update; `outcome` `success\|failed\|noop\|refused`, `failed_stage` `download\|verify\|apply\|restart`, `from_commit_date` epoch seconds. Source-checkout `hermes update` runs are counted from their receipt, never here. Empty optional strings and `None` numbers are omitted. Always `{ok: true}`. Electron: `reportPendingUpdateRun`. |
| Feature area opened | `desktop_feature_use_call` / `desktop_feature_use` | `shared_metrics.desktop_feature_use` `{area, profile?}` | **Telemetry.** Once per (profile, UTC day, area); unknown ids collapse to `other`. Always `{ok: true}`. Electron: `recordFeatureUse` (`store/desktop-metrics.ts`). |
| Friction event | `desktop_friction_call` / `desktop_friction` | `shared_metrics.desktop_friction` `{kind, detail, profile?}` | **Telemetry.** `kind` `notice_dismissed\|error_toast\|renderer_crash\|backend_disconnect\|slow_frame`; `detail` a closed word for that kind (never message text), capped per day. Always `{ok: true}`. Electron: `recordFriction`. |
| Onboarding step | `desktop_onboarding_call` / `desktop_onboarding` | `shared_metrics.desktop_onboarding` `{step, event, profile?}` | **Telemetry.** `event` `reached\|completed\|abandoned`, once per (step, event) per profile. Always `{ok: true}`. Electron: `recordOnboarding`. |
| Dislike signal | `desktop_dislike_call` / `desktop_dislike` | `shared_metrics.desktop_dislike` `{signal, target, setting?, profile?}` | **Telemetry.** `signal` `quick_close\|cancelled\|setting_off_default\|rage_click\|undo\|feature_disabled`, capped per signal per day. For `setting_off_default` pass `setting` (config key): the builder mirrors Electron and sends `target: "setting"` + the key only — the backend compares the saved value to the default, the value is never sent. Always `{ok: true}`. Electron: `recordDislike`. |
| Daily rollup | `desktop_daily_call` / `desktop_daily` | `shared_metrics.desktop_daily` `{day, bot_count, modes, actions, profile?}` | **Telemetry.** One finished UTC day; `day` (`YYYY-MM-DD`) only latches a resend and is never recorded. `modes` = `DesktopModeDay { mode: "bots"\|"sessions", active_ms, messages_sent }`, `actions` = `DesktopActionDay { action, via: click\|menu\|palette\|shortcut, count }`. Reply `{recorded}`: true = settled, drop the buffered day; false = keep it. Electron: `flushPendingDays`. |

Reply types are lenient (`#[serde(default)]`): `SharedMetricsConsent`, `OkReply`,
`DesktopDailyResult`. `{}` decodes to defaults for all three, so a sparse reply never fails.

## Safety

* `set` is the **only** call that changes state the user owns: it writes
  `telemetry.shared_metrics.{enabled,send}` in the focused profile's `config.yaml`. Show the
  answer the user gave; never call it to pre-check, reset or silently default consent.
* Every report is telemetry. The handlers record nothing when the profile's collection flag is
  off (`shared_metrics_events._emit` → `relay_shared_metrics.enabled()`;
  `record_desktop_feature_use`/`record_desktop_daily` gate the same way), and once collection is
  on a daily upload may carry the records off the machine. Fire-and-forget: swallow errors, as
  the Electron callers do.
* Nothing here spends model calls or reveals secrets.

## Run live (private test backend)

`crates/hermes-core/tests/live_api_metrics.rs` — 2 checks, run against
`HERMES_DESKTOP_PORT=<port>` (`scripts/live-test-backend.sh`):

* RPC `shared_metrics.status` → decodes, and `send` is never true while `enabled` is false
  (logged: `enabled=false send=false decided=true reask=false`).
* the same reply's flags are coherent: never `decided` and `reask` together.

## Not run live (and why)

| Endpoint | Why not |
|---|---|
| `shared_metrics.set` | Changes the user's telemetry consent in `config.yaml` (and purges latches/receipts when disabling). |
| `shared_metrics.slash_command`, `.startup_latency`, `.update_run`, `.desktop_feature_use`, `.desktop_friction`, `.desktop_onboarding`, `.desktop_dislike`, `.desktop_daily` | Record telemetry that a later upload can send off the machine. The handlers gate on collection — the live test confirmed consent is off, so they would be no-ops — but the area's live scope is the consent read only; they stay builder + unit-tested. |

## Notes on the contract

* The desktop's `desktop_daily` days are clamped by the backend to a rolling window
  (`DAILY_REPORT_WINDOW_DAYS`); older or future `day` values are treated as settled
  (`recorded: true`) without recording, so the client stops resending them.
* `shared_metrics.status` answers the focused profile exactly as written: `decided` is false
  when neither key exists in `config.yaml`, even if the shipped defaults say off.
