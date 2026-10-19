//! Desktop telemetry: `shared_metrics.*` RPCs (consent state and the desktop's usage reports).
//!
//! Two families, both `_profile_scoped` on the backend (`tui_gateway/methods_shared_metrics.py`),
//! so every builder takes a `profile` (`""` = whatever profile the connection is focused on):
//!
//! * **Consent** — [`status`] reads `telemetry.shared_metrics.{enabled,send}` from the profile's
//!   `config.yaml`; [`set`] writes both opt-ins through the same single writer `hermes setup`
//!   uses. `send` cannot outlive `enabled`: the backend forces it off when collection is turned
//!   off, withdraws the send-consent window, purges onboarding latches and pending receipts.
//! * **Usage reports** — one fire-and-forget call per Desktop event: a slash command typed, the
//!   launch-to-ready latency, a packaged self-update run, a feature area opened, a friction
//!   event, an onboarding step transition, a dislike signal and one finished UTC day of use.
//!   Every one of them is recorded **only when shared metrics are enabled for the profile**
//!   (`hermes_cli/observability/shared_metrics_desktop.py` gates on `relay_shared_metrics.enabled()`
//!   and `_emit` no-ops otherwise), and all but [`desktop_daily`] answer `{ok: true}` no matter
//!   what. They are telemetry: once collected, a daily upload may carry them off the machine.
//!
//! The Electron desktop calls each method from exactly one place — recorded per function below
//! and in `docs/api/metrics.md`. Replies are lenient (`#[serde(default)]`), so a missing or extra
//! field never fails a decode, and an older backend that lacks a method surfaces as
//! [`ApiError::Rpc`] (the Electron UI disables the controls instead).

use serde_json::{Value, json};

use crate::api::{ApiError, Call, call_as, call_of};
use crate::rpc::Client;

/// Add `profile` to a params object only when it names one; the backend then scopes the call to
/// that profile. Empty leaves the connection's focused profile in charge.
fn scoped(mut params: Value, profile: &str) -> Value {
    if !profile.is_empty() {
        params["profile"] = json!(profile);
    }
    params
}

// ── Consent ─────────────────────────────────────────────────────────────────────

/// `shared_metrics.status` / `shared_metrics.set` reply: the focused profile's opt-ins.
///
/// `send` is never true while `enabled` is false. `decided` means either key is written in
/// `config.yaml` (the shipped defaults are not an answer); `reask` is an "off" from before the
/// type-ahead fix, offered once more with the reason. `{}` decodes to all-false, which every
/// caller treats as "don't ask" (the Electron `readSharedMetricsConsent` returns `null` on a
/// failed read and gates the first-run offer on `decided == false`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SharedMetricsConsent {
    /// Local collection (`telemetry.shared_metrics.enabled`).
    pub enabled: bool,
    /// Daily upload (`telemetry.shared_metrics.send`); forced false when `enabled` is false.
    pub send: bool,
    /// The user answered (either key written); `reask` answers still count as undecided.
    pub decided: bool,
    /// An old "off" that deserves one more offer; never true together with `decided`.
    pub reask: bool,
}

/// The profile's consent, straight from its `config.yaml` — a pure read, no defaults merged.
pub fn status_call(profile: &str) -> Call {
    call_of("shared_metrics.status", scoped(json!({}), profile))
}

/// Read the profile's shared-metrics consent. **Read-only.**
pub async fn status(client: &Client, profile: &str) -> Result<SharedMetricsConsent, ApiError> {
    call_as(client, &status_call(profile)).await
}

/// Write both opt-ins at once (the wizard's invariant: sending cannot outlive collection, so
/// `send` is ignored unless `enabled`). `first_run` marks the Desktop first-run answer, which
/// also records the `setup_completed` metric through the events API.
///
/// **Changes the user's telemetry consent in `config.yaml`** and reconciles the local store:
/// turning collection off withdraws send consent, purges onboarding latches and drops pending
/// upload receipts. Call only from an explicit user answer; never to pre-check or reset consent.
pub fn set_call(profile: &str, enabled: bool, send: bool, first_run: bool) -> Call {
    call_of(
        "shared_metrics.set",
        scoped(
            json!({ "enabled": enabled, "send": send, "first_run": first_run }),
            profile,
        ),
    )
}

/// `shared_metrics.set` deadline: the pooled handler waits on the metrics store's write lock.
pub const CONSENT_WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);

/// Write the profile's consent (see [`set_call`]). **Changes telemetry consent.**
pub async fn set(
    client: &Client,
    profile: &str,
    enabled: bool,
    send: bool,
    first_run: bool,
) -> Result<SharedMetricsConsent, ApiError> {
    // Pooled handler (`_LONG_HANDLERS`): consent reconcile waits on the metrics store's write
    // lock, which can outlast the 30 s default under contention.
    let c = set_call(profile, enabled, send, first_run);
    let v = client
        .request_with_timeout(c.method, c.params, CONSENT_WRITE_TIMEOUT)
        .await?;
    crate::api::decode(v)
}

/// `{ok: true}` reply of every report below except [`desktop_daily`]. A missing `ok` decodes as
/// `false`, so read the `Result`, not the flag, to detect a failed call.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct OkReply {
    pub ok: bool,
}

// ── Usage reports ───────────────────────────────────────────────────────────────

/// One slash command the user typed (Electron: `use-prompt-actions/slash.ts`, once per typed
/// command whether the desktop handles it locally or forwards it to the gateway). `command` is
/// the bare name — no leading `/`, no arguments; the backend canonicalizes it against the
/// published registry. `session_id` scopes the count to that session's profile when it is known.
pub fn slash_command_call(profile: &str, command: &str, session_id: Option<&str>) -> Call {
    let mut params = json!({ "command": command });
    if let Some(id) = session_id.filter(|s| !s.is_empty()) {
        params["session_id"] = json!(id);
    }
    call_of("shared_metrics.slash_command", scoped(params, profile))
}

/// Report one typed slash command. **Telemetry** (recorded only with collection on; may be
/// uploaded); fire-and-forget, always `{ok: true}`.
pub async fn slash_command(
    client: &Client,
    profile: &str,
    command: &str,
    session_id: Option<&str>,
) -> Result<OkReply, ApiError> {
    call_as(client, &slash_command_call(profile, command, session_id)).await
}

/// Who is reporting a launch latency. The client declares it because a Desktop attached to a
/// URL/cloud backend has no `HERMES_DESKTOP` for the backend to detect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupSurface {
    /// The Desktop app start → backend attached.
    DesktopAttach,
    /// The Ink TUI process start → gateway ready.
    Tui,
}

impl StartupSurface {
    pub fn as_str(self) -> &'static str {
        match self {
            StartupSurface::DesktopAttach => "desktop_attach",
            StartupSurface::Tui => "tui",
        }
    }
}

/// The client's own launch → ready time, measured once per launch (Electron:
/// `app/gateway/report-startup-latency.ts`, once per app launch from the main-process claim).
/// The backend latches on `launch_id` (an opaque per-launch token, never recorded) so a
/// reconnect re-sending the same launch counts once while a new launch counts again.
pub fn startup_latency_call(profile: &str, elapsed_ms: f64, surface: StartupSurface, launch_id: &str) -> Call {
    call_of(
        "shared_metrics.startup_latency",
        scoped(
            json!({
                "elapsed_ms": elapsed_ms,
                "launch_id": launch_id,
                "surface": surface.as_str(),
            }),
            profile,
        ),
    )
}

/// Report one launch → ready latency. **Telemetry**; fire-and-forget, always `{ok: true}`.
pub async fn startup_latency(
    client: &Client,
    profile: &str,
    elapsed_ms: f64,
    surface: StartupSurface,
    launch_id: &str,
) -> Result<OkReply, ApiError> {
    call_as(client, &startup_latency_call(profile, elapsed_ms, surface, launch_id)).await
}

/// One desktop **packaged** self-update run (Electron: `store/shared-metrics.ts`
/// `reportPendingUpdateRun`, records persisted by the main process survive the restart, sent
/// once on the next backend attach). Raw words; the backend buckets them:
/// `outcome` `success|failed|noop|refused`; `failed_stage` `download|verify|apply|restart`;
/// `mechanism` the updater strategy kind; `duration_ms` wall time; `from_commit_date` the
/// updated-from build's commit time in epoch seconds. Source-checkout `hermes update` runs are
/// counted from their receipt and must not be reported here.
pub fn update_run_call(
    profile: &str,
    outcome: &str,
    failed_stage: Option<&str>,
    mechanism: Option<&str>,
    duration_ms: Option<f64>,
    from_commit_date: Option<f64>,
) -> Call {
    let mut params = json!({ "outcome": outcome });
    for (key, value) in [("failed_stage", failed_stage), ("mechanism", mechanism)] {
        if let Some(v) = value.filter(|s| !s.is_empty()) {
            params[key] = json!(v);
        }
    }
    for (key, value) in [("duration_ms", duration_ms), ("from_commit_date", from_commit_date)] {
        if let Some(v) = value {
            params[key] = json!(v);
        }
    }
    call_of("shared_metrics.update_run", scoped(params, profile))
}

/// Report one packaged self-update run (see [`update_run_call`]). **Telemetry**;
/// fire-and-forget, always `{ok: true}`.
#[allow(clippy::too_many_arguments)]
pub async fn update_run(
    client: &Client,
    profile: &str,
    outcome: &str,
    failed_stage: Option<&str>,
    mechanism: Option<&str>,
    duration_ms: Option<f64>,
    from_commit_date: Option<f64>,
) -> Result<OkReply, ApiError> {
    call_as(
        client,
        &update_run_call(profile, outcome, failed_stage, mechanism, duration_ms, from_commit_date),
    )
    .await
}

/// One Desktop surface the user opened (Electron: `store/desktop-metrics.ts`
/// `recordFeatureUse`), at most once per UTC day; `area` is a closed Desktop surface id
/// (`command_palette`, `terminal_pane`, `settings_<view>` …) and the backend collapses anything
/// outside its set to `other`.
pub fn desktop_feature_use_call(profile: &str, area: &str) -> Call {
    call_of(
        "shared_metrics.desktop_feature_use",
        scoped(json!({ "area": area }), profile),
    )
}

/// Report a feature area opened. **Telemetry**; fire-and-forget, always `{ok: true}`.
pub async fn desktop_feature_use(client: &Client, profile: &str, area: &str) -> Result<OkReply, ApiError> {
    call_as(client, &desktop_feature_use_call(profile, area)).await
}

/// One Desktop friction event. `kind` is `notice_dismissed|error_toast|renderer_crash|
/// backend_disconnect|slow_frame`; `detail` a closed code-defined word for that kind (notice id,
/// error category, crash reason, drop reason, frame-duration bucket) — never message text.
pub fn desktop_friction_call(profile: &str, kind: &str, detail: &str) -> Call {
    call_of(
        "shared_metrics.desktop_friction",
        scoped(json!({ "kind": kind, "detail": detail }), profile),
    )
}

/// Report one friction event. **Telemetry**; fire-and-forget, always `{ok: true}`.
pub async fn desktop_friction(client: &Client, profile: &str, kind: &str, detail: &str) -> Result<OkReply, ApiError> {
    call_as(client, &desktop_friction_call(profile, kind, detail)).await
}

/// One Desktop first-run step transition. `step` is a Desktop first-run step id; `event` is
/// `reached|completed|abandoned`. Recorded once per (step, event) per profile.
pub fn desktop_onboarding_call(profile: &str, step: &str, event: &str) -> Call {
    call_of(
        "shared_metrics.desktop_onboarding",
        scoped(json!({ "step": step, "event": event }), profile),
    )
}

/// Report one onboarding step transition. **Telemetry**; fire-and-forget, always `{ok: true}`.
pub async fn desktop_onboarding(client: &Client, profile: &str, step: &str, event: &str) -> Result<OkReply, ApiError> {
    call_as(client, &desktop_onboarding_call(profile, step, event)).await
}

/// One Desktop dislike signal. `signal` is `quick_close|cancelled|setting_off_default|
/// rage_click|undo|feature_disabled`; `target` a closed code-defined id for that signal (area,
/// flow, action, undo path, feature toggle). For `setting_off_default` pass `setting` (a config
/// key) instead of a target: like the Electron caller, the builder then sends `target: "setting"`
/// and the key **only** — the saved value is compared to the default by the backend, never sent.
pub fn desktop_dislike_call(profile: &str, signal: &str, target: &str, setting: Option<&str>) -> Call {
    let mut params = json!({ "signal": signal });
    // The backend reads `setting` only for `setting_off_default`; any other signal keeps its
    // own target (shared_metrics_desktop.py), so `setting` is dropped there.
    match setting.filter(|s| !s.is_empty() && signal == "setting_off_default") {
        Some(key) => {
            params["setting"] = json!(key);
            params["target"] = json!("setting");
        }
        None => params["target"] = json!(target),
    }
    call_of("shared_metrics.desktop_dislike", scoped(params, profile))
}

/// Report one dislike signal, capped per signal per day. **Telemetry**; fire-and-forget,
/// always `{ok: true}`.
pub async fn desktop_dislike(
    client: &Client,
    profile: &str,
    signal: &str,
    target: &str,
    setting: Option<&str>,
) -> Result<OkReply, ApiError> {
    call_as(client, &desktop_dislike_call(profile, signal, target, setting)).await
}

/// One mode's use on a reported day (`mode` `bots` | `sessions`).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct DesktopModeDay {
    pub mode: String,
    /// Active time in milliseconds (the backend buckets it).
    pub active_ms: f64,
    pub messages_sent: u64,
}

/// One `(action, via)` button count on a reported day; `via` is `click|menu|palette|shortcut`.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct DesktopActionDay {
    pub action: String,
    pub via: String,
    pub count: u64,
}

/// `shared_metrics.desktop_daily` reply: `recorded: true` once the day is settled (saved,
/// already reported, empty or out of range), so the client drops its copy; `false` keeps it
/// for the next attach — including when collection is off, where the client's own gate decides.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct DesktopDailyResult {
    pub recorded: bool,
}

/// One finished UTC day of Desktop use, aggregated on the client (Electron: `store/
/// desktop-metrics.ts` `flushPendingDays`, one call per pending day, oldest first). `day`
/// (`YYYY-MM-DD`) only latches a resend and is never recorded; `modes` and `actions` are the raw
/// counts, bucketed by the backend. Days outside its rolling window are dropped as `recorded`.
pub fn desktop_daily_call(
    profile: &str,
    day: &str,
    modes: &[DesktopModeDay],
    actions: &[DesktopActionDay],
    bot_count: u64,
) -> Call {
    call_of(
        "shared_metrics.desktop_daily",
        scoped(
            json!({
                "day": day,
                "bot_count": bot_count,
                "modes": modes,
                "actions": actions,
            }),
            profile,
        ),
    )
}

/// Report one finished Desktop day. **Telemetry** (a batch of usage counts; may be uploaded).
/// Unlike the other reports this one answers `{recorded: bool}`: drop the buffered day only
/// when `recorded` is true.
pub async fn desktop_daily(
    client: &Client,
    profile: &str,
    day: &str,
    modes: &[DesktopModeDay],
    actions: &[DesktopActionDay],
    bot_count: u64,
) -> Result<DesktopDailyResult, ApiError> {
    call_as(client, &desktop_daily_call(profile, day, modes, actions, bot_count)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consent_calls() {
        let c = status_call("");
        assert_eq!(c.method, "shared_metrics.status");
        assert_eq!(c.params, json!({}));
        assert_eq!(status_call("swe").params, json!({ "profile": "swe" }));

        let c = set_call("", true, false, false);
        assert_eq!(c.method, "shared_metrics.set");
        assert_eq!(c.params, json!({ "enabled": true, "send": false, "first_run": false }));
        assert_eq!(
            set_call("swe", false, true, true).params,
            json!({ "enabled": false, "send": true, "first_run": true, "profile": "swe" })
        );
    }

    #[test]
    fn consent_decode() {
        let c: SharedMetricsConsent = serde_json::from_value(json!({
            "enabled": true, "send": true, "decided": true, "reask": false, "x": 1
        }))
        .unwrap();
        assert!(c.enabled && c.send && c.decided && !c.reask);
        let empty: SharedMetricsConsent = serde_json::from_value(json!({})).unwrap();
        assert_eq!(empty, SharedMetricsConsent::default());
        let reask: SharedMetricsConsent = serde_json::from_value(json!({
            "enabled": false, "send": false, "decided": false, "reask": true
        }))
        .unwrap();
        assert!(reask.reask && !reask.decided);
    }

    #[test]
    fn slash_command_calls() {
        let c = slash_command_call("", "model", None);
        assert_eq!(c.method, "shared_metrics.slash_command");
        assert_eq!(c.params, json!({ "command": "model" }));

        let c = slash_command_call("swe", "compact", Some("sess-1"));
        assert_eq!(
            c.params,
            json!({ "command": "compact", "session_id": "sess-1", "profile": "swe" })
        );
        // An empty session id is omitted rather than sent as "".
        assert_eq!(
            slash_command_call("", "help", Some("")).params,
            json!({ "command": "help" })
        );
    }

    #[test]
    fn startup_latency_calls() {
        let c = startup_latency_call("", 1234.5, StartupSurface::DesktopAttach, "launch-1");
        assert_eq!(c.method, "shared_metrics.startup_latency");
        assert_eq!(
            c.params,
            json!({
                "elapsed_ms": 1234.5, "launch_id": "launch-1", "surface": "desktop_attach"
            })
        );
        assert_eq!(
            startup_latency_call("swe", 88.0, StartupSurface::Tui, "l2").params,
            json!({
                "elapsed_ms": 88.0, "launch_id": "l2", "surface": "tui", "profile": "swe"
            })
        );
        assert_eq!(StartupSurface::DesktopAttach.as_str(), "desktop_attach");
        assert_eq!(StartupSurface::Tui.as_str(), "tui");
    }

    #[test]
    fn update_run_calls() {
        let c = update_run_call("", "success", None, None, None, None);
        assert_eq!(c.method, "shared_metrics.update_run");
        assert_eq!(c.params, json!({ "outcome": "success" }));

        let c = update_run_call(
            "swe",
            "failed",
            Some("verify"),
            Some("electron-updater"),
            Some(42_000.0),
            Some(1_700_000_000.0),
        );
        assert_eq!(
            c.params,
            json!({
                "outcome": "failed", "failed_stage": "verify", "mechanism": "electron-updater",
                "duration_ms": 42000.0, "from_commit_date": 1700000000.0, "profile": "swe"
            })
        );
        // Empty strings are omitted rather than sent as "".
        assert_eq!(
            update_run_call("", "noop", Some(""), Some(""), None, None).params,
            json!({ "outcome": "noop" })
        );
    }

    #[test]
    fn desktop_report_calls() {
        let c = desktop_feature_use_call("", "terminal_pane");
        assert_eq!(c.method, "shared_metrics.desktop_feature_use");
        assert_eq!(c.params, json!({ "area": "terminal_pane" }));

        let c = desktop_friction_call("swe", "error_toast", "network");
        assert_eq!(c.method, "shared_metrics.desktop_friction");
        assert_eq!(
            c.params,
            json!({ "kind": "error_toast", "detail": "network", "profile": "swe" })
        );

        let c = desktop_onboarding_call("", "consent", "completed");
        assert_eq!(c.method, "shared_metrics.desktop_onboarding");
        assert_eq!(c.params, json!({ "step": "consent", "event": "completed" }));
    }

    #[test]
    fn desktop_dislike_calls() {
        let c = desktop_dislike_call("", "quick_close", "palette", None);
        assert_eq!(c.method, "shared_metrics.desktop_dislike");
        assert_eq!(c.params, json!({ "signal": "quick_close", "target": "palette" }));

        // A setting forced by the user: key + direction only, value never sent.
        let c = desktop_dislike_call("swe", "setting_off_default", "ignored", Some("display.show_reasoning"));
        assert_eq!(
            c.params,
            json!({
                "signal": "setting_off_default", "target": "setting",
                "setting": "display.show_reasoning", "profile": "swe"
            })
        );
        // An empty setting falls back to the plain target form.
        assert_eq!(
            desktop_dislike_call("", "undo", "message", Some("")).params,
            json!({ "signal": "undo", "target": "message" })
        );
        // `setting` belongs to `setting_off_default` only; other signals keep their target.
        assert_eq!(
            desktop_dislike_call("", "undo", "closed_tab", Some("display.x")).params,
            json!({ "signal": "undo", "target": "closed_tab" })
        );
    }

    #[test]
    fn desktop_daily_calls() {
        let modes = [DesktopModeDay {
            mode: "bots".into(),
            active_ms: 12_345.0,
            messages_sent: 3,
        }];
        let actions = [DesktopActionDay {
            action: "new_session".into(),
            via: "palette".into(),
            count: 2,
        }];
        let c = desktop_daily_call("", "2026-10-18", &modes, &actions, 4);
        assert_eq!(c.method, "shared_metrics.desktop_daily");
        assert_eq!(
            c.params,
            json!({
                "day": "2026-10-18",
                "bot_count": 4,
                "modes": [{ "mode": "bots", "active_ms": 12345.0, "messages_sent": 3 }],
                "actions": [{ "action": "new_session", "via": "palette", "count": 2 }],
            })
        );
        assert_eq!(
            desktop_daily_call("swe", "2026-10-18", &[], &[], 0).params,
            json!({
                "day": "2026-10-18", "bot_count": 0, "modes": [], "actions": [],
                "profile": "swe"
            })
        );

        let r: DesktopDailyResult = serde_json::from_value(json!({ "recorded": true })).unwrap();
        assert!(r.recorded);
        assert_eq!(
            serde_json::from_value::<DesktopDailyResult>(json!({})).unwrap(),
            DesktopDailyResult::default()
        );
    }

    #[test]
    fn ok_reply_decode() {
        let ok: OkReply = serde_json::from_value(json!({ "ok": true })).unwrap();
        assert!(ok.ok);
        assert_eq!(
            serde_json::from_value::<OkReply>(json!({ "ok": true, "extra": 1 })).unwrap(),
            OkReply { ok: true }
        );
        // A backend that omits `ok` must not fail the decode.
        assert!(!serde_json::from_value::<OkReply>(json!({})).unwrap().ok);
    }
}
