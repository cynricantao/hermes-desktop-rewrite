# Notifications

The notification rules the desktop runs before anything is shown: which events raise an
in-app toast, which raise a desktop (OS) notification, and what each one says. Pure logic in
`crates/hermes-core/src/notify.rs`; the controller performs the side effects. Ported from the
Electron desktop (see the module docs for the exact source files).

Nothing in this module contacts a notification daemon — no D-Bus, no `notify-send`, no
Electron `Notification`. Tests are plain unit tests.

## Event kinds

`NativeNotificationKind` — one settings toggle each (`NATIVE_NOTIFICATION_KINDS` in
`src/store/native-notifications.ts`):

| Kind | Token | Fires as | Settings label |
|---|---|---|---|
| Approval | `approval` | attention | Approval needed |
| Input | `input` | attention | Input needed |
| TurnDone | `turnDone` | completion | Response ready |
| TurnError | `turnError` | completion | Turn failed |
| BackgroundDone | `backgroundDone` | completion | Background task finished |
| Credits | `credits` | global | Credit alerts |
| Plugin | `plugin` | global (tagged) | Plugin notifications |

Attention kinds (approval, input) are the two that break through while the window is focused
if they belong to another session. Global events (credit notices, plugin notifications, pet
generation) are not tied to a chat session and fire whenever the user is away.

## The decision

```rust
pub fn should_notify(
    event: &NotificationEvent,
    prefs: &NativeNotificationPrefs,
    window_focused: bool,
    active_session: Option<&str>,
) -> NotifySurface   // None | Toast | Desktop | Both
```

Electron's rules, in order:

1. **Preferences** — the master switch or the event's kind silences the desktop half; the
   in-app toast half is the caller's intent (`NotificationEvent::wants_toast`) and is not
   gated (the toast feed has no settings in Electron either).
2. **Post-connect quiet window** — `NativeNotifyBaseline` (4 s, `SEED_QUIET_MS`) opens on
   every gateway `open`; events marked `in_baseline` skip the desktop half, so a replayed
   parked approval does not raise an OS notification on launch or reconnect. The in-app
   surfaces still show it immediately.
3. **Focus/session gate** (`desktop_gate`, Electron's `shouldFire`):
   * global → fires when the window is unfocused;
   * attention → fires when unfocused, or while focused if the event has a session that is
     not the active one;
   * completion → fires only when unfocused **and** the event's session is the active one
     (so a busy gateway cannot spam a toast per background session).

`should_fire_desktop` is the boolean form (steps 1–3); `desktop_gate` is the bare focus gate.
`window_focused` mirrors `isBackgrounded()` — Electron treats a visible-but-unfocused window
(alt-tab) as away, and only `document.hasFocus()` matters.

## Throttle and delivery dedupe

| Rule | Value | Source |
|---|---|---|
| Renderer throttle key | `kind:sessionId ?? tag ?? (global ? "global" : "")` | `native-notifications.ts` |
| Renderer throttle window | 1 s (`THROTTLE_MS`), self-evicting | same |
| IPC dedupe key | `kind:sessionId ?? tag ?? ""` | `electron/notification-ipc.ts` |
| IPC dedupe window | 1 s (`DELIVERY_DEDUPE_MS`) | same |
| Notification retention TTL | 10 min (`NOTIFICATION_RETENTION_TTL_MS`) | `notification-registry.ts` |

`NativeThrottle` and `DeliveryDedupe` are small state holders; both prune on every call.
Plugin notifications are throttled per plugin id (`tag`), so two plugins cannot collapse each
other's notifications.

## Native copy

Per-kind titles and bodies (`notifications.native.*` in `src/i18n/en.ts`, shipped here as
constants):

| Event | Title | Body |
|---|---|---|
| Approval | `Approval needed` / `Approval needed — <session>` | the command or description; buttons Approve/Reject |
| Input | `Input needed` / `Input needed — <session>` | `Hermes is waiting for your response.` (or the request body) |
| TurnDone | `Hermes finished` | reply text, else `Message complete.`, capped at 140 chars |
| TurnError | `Turn failed` | the error's toast message |
| BackgroundDone | `Background task finished` / `Background task failed` | title + `(exit N)` + last 3 output lines |
| Credits | `Credits` | the notice text |
| Plugin | caller-provided | caller-provided |

Helpers: `approval_title` / `input_title` (session-named variants), `turn_done_body`,
`background_done_body`, `session_display_name` (title → preview → `#idtail`, 80-char
ellipsis), `approval_actions` (buttons carry `approve:<requestId>` / `reject:<requestId>`)
and `parse_approval_action_id` (+ `ApprovalChoice::rpc_choice()` → `once` / `deny`). The
settings "send test" (`TEST_NOTIFY_TITLE` / `TEST_NOTIFY_BODY`) bypasses every gate so the
panel can flag a silent permission failure.

## In-app toasts

`ToastFeed` is the pure form of `$notifications` in `src/store/notifications.ts`:

* **Lifetime** — `toast_default_duration_ms`: 0 (sticky) for error/warning, 5 000 ms
  otherwise; `Toast::from_input` honours a per-toast `duration_ms` override. `due_ids(now)`
  returns the expired ones; sticky toasts never appear.
* **Dedup** — a repeat with the same id replaces the previous toast, moves it to the front
  and restarts its lifetime (`push`). Ids fall back to `<now>-<seq>`.
* **Placement** — `toast_default_placement`: errors, warnings and anything with an action
  button go to the top-center slot (`Default`); routine confirmations stay bottom-right.
* **Stacking** — the feed keeps everything; visual depth is the view's job. `visible_len` /
  `hidden_len` model the collapsed (front card + "N more notifications", `toast_more_label`)
  vs expanded stack.

## Error summarising

`readable_error(raw, fallback)` ports `readableError` + `ERROR_SUMMARIES`: unwrap the IPC
wrapper (`Error invoking remote method '…': Error: …`), strip a leading `Error:`, prefer an
embedded `"detail"`, then match the rule table — disk full / ENOSPC, classified storage
codes (`storage_*`, `disk_full` via `extract_error_code`), `gateway_auth_failed`,
`invalid_api_key` / "incorrect API key provided", missing OpenAI / ElevenLabs keys,
ElevenLabs HTTP 401, "method not allowed", microphone permission, "out of sync (different
versions)", "Restart required:". Each rule yields an `ErrorToastCategory` (telemetry), the
shipped English message and an optional `RecoveryAction` whose `route()` is the settings deep
link (`/settings?tab=keys&key=…`, `/settings?tab=gateway`, `/command-center?section=maintenance`).
Unclassified messages longer than 180 chars (or empty) fall back to the caller's text.
`ErrorToastCategory::Timeout` is never produced here: callers flip `Unclassified` when their
error is a timeout, mirroring `notifyError`.

## Settings-page identity

`NOTIFICATIONS_SETTINGS_TAB` (`notifications`), subpages `alerts` / `sounds`,
`ENABLE_ALL_SETTING_ID` (`notifications.enable-all`) and
`NativeNotificationKind::setting_id()` (`notifications.kind-<kind>`) match
`settings-manifest.ts`. Preference storage uses `NATIVE_NOTIFY_STORAGE_KEY`
(`hermes:native-notifications`) with the same lenient decoding as `readPrefs`
(`NativeNotificationPrefs::from_stored_json` / `to_stored_json`).

## Linux presenter facts

Ported constants for whoever implements the D-Bus side: app name `Hermes`, desktop entry
`hermes`, urgency 1 (normal), 5 s delivery timeout, 10 s retry cooldown after a failure,
max 32 pending deliveries. `linux_actions` builds `['default', 'View', '0', text0, …]` only
when the daemon advertises `actions` and `ELECTRON_USE_UBUNTU_NOTIFIER` is unset.

## Not ported

Friction telemetry, `desktop.log` writes, haptics, icon-path resolution, the completion-sound
picker, and all actual Electron/D-Bus delivery. The click handlers (focus-session,
notification-activate) are controller concerns; this module models the payload only.

## Tests

Unit tests live in the same file (`cargo test -p hermes-core notify`), including the
`native-notifications.test.ts` decision matrix (focused/unfocused, active/other session,
global, baseline, prefs off), throttle and IPC-dedupe windows, Linux action assembly, toast
lifecycle/dedup/stacking, and one case per error rule.
