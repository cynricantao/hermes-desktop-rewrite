//! Notification rules: which events raise an in-app toast and which raise a desktop
//! (OS) notification, given the user's settings and window state. UI-free — the
//! desktop app's controller calls these functions and performs the side effects
//! (Slint toast rows, the platform notification daemon).
//!
//! Ported from the Electron desktop (behaviour, not code):
//! * `src/store/native-notifications.ts` — event kinds, preference defaults and the
//!   `hermes:native-notifications` storage key, the focus/session gate (`shouldFire`),
//!   the 1 s throttle/dedupe, plugin notifications and the per-kind titles/actions.
//! * `src/store/notify-baseline.ts` — the 4 s post-connect quiet window.
//! * `src/store/notifications.ts` — the in-app toast feed: durations, placements,
//!   replace-by-id dedup, the error summariser (`readableError`) and recovery actions.
//! * `electron/notification-ipc.ts`, `electron/notification-registry.ts`,
//!   `electron/notification-linux.ts` — main-process delivery dedupe, notification
//!   retention and the Linux (D-Bus `org.freedesktop.Notifications`) presenter constants.
//! * `src/app/settings/notifications-settings.tsx` + `src/i18n/en.ts` — the settings
//!   rows that gate kinds, the settings deep-link ids and the English copy shipped here
//!   as defaults.
//!
//! Not ported (side effects, not rules): friction telemetry, `desktop.log` writes,
//! haptics, icon-path resolution, and the actual Electron/D-Bus calls. Nothing in this
//! module contacts a notification daemon.

use std::collections::{BTreeMap, HashMap};
use std::fmt;

use serde::{Deserialize, Serialize};

// ── Event kinds ────────────────────────────────────────────────────────────────

/// A kind of desktop (OS) notification. One setting toggles each kind
/// (`NATIVE_NOTIFICATION_KINDS` in `src/store/native-notifications.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NativeNotificationKind {
    /// A command is waiting for the user to approve or reject it.
    Approval,
    /// Hermes asked a question, or needs a password/secret.
    Input,
    /// A turn finished while Hermes was in the background.
    TurnDone,
    /// A background turn failed.
    TurnError,
    /// A backgrounded terminal command completed.
    BackgroundDone,
    /// Credit access is paused or restored.
    Credits,
    /// A desktop plugin sent a notification while Hermes was in the background.
    Plugin,
}

impl NativeNotificationKind {
    /// Every kind, in the order the settings page lists them.
    pub const ALL: [Self; 7] = [
        Self::Approval,
        Self::Input,
        Self::TurnDone,
        Self::TurnError,
        Self::BackgroundDone,
        Self::Credits,
        Self::Plugin,
    ];

    /// Wire/storage token (`approval`, `turnDone`, …).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Approval => "approval",
            Self::Input => "input",
            Self::TurnDone => "turnDone",
            Self::TurnError => "turnError",
            Self::BackgroundDone => "backgroundDone",
            Self::Credits => "credits",
            Self::Plugin => "plugin",
        }
    }

    /// Parse a wire/storage token; `None` for anything else.
    pub fn from_token(token: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == token)
    }

    /// Blocking prompts: they surface even while focused if they are for another
    /// session (`ATTENTION_KINDS` in the Electron store).
    pub const fn is_attention(self) -> bool {
        matches!(self, Self::Approval | Self::Input)
    }

    /// The settings-page row id of this kind's toggle
    /// (`notifications.kind-<kind>`, from `settings-manifest.ts`).
    pub fn setting_id(self) -> String {
        format!("notifications.kind-{}", self.as_str())
    }
}

impl fmt::Display for NativeNotificationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── Settings ───────────────────────────────────────────────────────────────────

/// Storage key of the native notification preferences
/// (`STORAGE_KEY` in `src/store/native-notifications.ts`).
pub const NATIVE_NOTIFY_STORAGE_KEY: &str = "hermes:native-notifications";

/// The settings rows that gate desktop notifications: one master switch plus one
/// toggle per kind. Defaults exactly as Electron ships them (all on).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NativeNotificationPrefs {
    /// Master switch; off silences every kind below.
    pub enabled: bool,
    /// Per-kind switches, keyed by [`NativeNotificationKind::as_str`].
    pub kinds: BTreeMap<String, bool>,
}

impl Default for NativeNotificationPrefs {
    fn default() -> Self {
        let kinds = NativeNotificationKind::ALL
            .into_iter()
            .map(|k| (k.as_str().to_owned(), true))
            .collect();
        Self { enabled: true, kinds }
    }
}

impl NativeNotificationPrefs {
    /// Whether `kind` is allowed. A missing entry means the shipped default (on),
    /// so a leniently decoded map can never silence a kind by accident.
    pub fn kind_enabled(&self, kind: NativeNotificationKind) -> bool {
        self.kinds.get(kind.as_str()).copied().unwrap_or(true)
    }

    /// Flip the master switch.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Flip one kind's switch.
    pub fn set_kind(&mut self, kind: NativeNotificationKind, on: bool) {
        self.kinds.insert(kind.as_str().to_owned(), on);
    }

    /// Decode the persisted JSON the way the renderer's `readPrefs` does: only
    /// booleans count, unknown kind keys are ignored, a missing master switch keeps
    /// its default, and corrupt storage falls back to the defaults.
    pub fn from_stored_json(raw: &str) -> Self {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
            return Self::default();
        };
        let enabled = value.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
        let mut kinds = Self::default().kinds;
        if let Some(stored) = value.get("kinds").and_then(|v| v.as_object()) {
            for kind in NativeNotificationKind::ALL {
                if let Some(on) = stored.get(kind.as_str()).and_then(|v| v.as_bool()) {
                    kinds.insert(kind.as_str().to_owned(), on);
                }
            }
        }
        Self { enabled, kinds }
    }

    /// Encode for storage (full record: master switch + every kind).
    pub fn to_stored_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

// ── The decision ───────────────────────────────────────────────────────────────

/// Where a notification should surface. `Toast` = in-app feed, `Desktop` = OS
/// notification, `Both` = both kinds of surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifySurface {
    /// Suppressed everywhere.
    None,
    /// In-app toast only.
    Toast,
    /// Desktop notification only.
    Desktop,
    /// In-app toast and desktop notification.
    Both,
}

impl NotifySurface {
    /// Whether the in-app toast feed should show this event.
    pub const fn toast(self) -> bool {
        matches!(self, Self::Toast | Self::Both)
    }

    /// Whether the platform notification daemon should be asked to show it.
    pub const fn desktop(self) -> bool {
        matches!(self, Self::Desktop | Self::Both)
    }
}

/// One notification-worthy event. The caller fills in what it knows; the decision
/// functions apply the Electron gates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationEvent {
    /// The event's kind.
    pub kind: NativeNotificationKind,
    /// Session this event belongs to, when it has one. Runtime id, as the renderer
    /// holds it (`NativeNotificationInput.sessionId`).
    pub session_id: Option<String>,
    /// Discriminator for session-less notifications (the plugin id), used for
    /// throttle/dedupe only — never for click-to-focus.
    pub tag: Option<String>,
    /// Not tied to a chat session (pet generation, credit notices): fires whenever
    /// the user is away, with no session-match requirement.
    pub global: bool,
    /// Whether the caller also wants an in-app toast. The toast feed itself has no
    /// focus gate in Electron; the caller's intent is the gate.
    pub wants_toast: bool,
    /// Set from [`NativeNotifyBaseline::within`]: events replayed right after a
    /// gateway socket opens must not raise an OS notification.
    pub in_baseline: bool,
}

impl NotificationEvent {
    /// A plain native-notification event of `kind` (no session, not global, no toast).
    pub fn native(kind: NativeNotificationKind) -> Self {
        Self {
            kind,
            session_id: None,
            tag: None,
            global: false,
            wants_toast: false,
            in_baseline: false,
        }
    }

    /// Attach the session the event belongs to.
    pub fn session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Mark the event session-less/account-wide.
    pub fn global(mut self) -> Self {
        self.global = true;
        self
    }

    /// Also show an in-app toast.
    pub fn toast(mut self) -> Self {
        self.wants_toast = true;
        self
    }

    /// Attach a throttle/dedupe tag (plugin id).
    pub fn tag(mut self, tag: impl Into<String>) -> Self {
        self.tag = Some(tag.into());
        self
    }

    /// Mark the event as replayed during the post-connect quiet window.
    pub fn in_baseline(mut self, in_baseline: bool) -> Self {
        self.in_baseline = in_baseline;
        self
    }
}

/// The decision function the UI calls: none / toast / desktop / both.
///
/// Electron rules (`shouldFire` + `dispatchNativeNotification` in
/// `src/store/native-notifications.ts`):
/// * a disabled master switch or kind silences the desktop half; the toast half is
///   the caller's intent and unaffected (the toast feed has no settings);
/// * replayed post-connect state ([`NativeNotifyBaseline`]) silences the desktop half;
/// * otherwise [`desktop_gate`] decides.
pub fn should_notify(
    event: &NotificationEvent,
    prefs: &NativeNotificationPrefs,
    window_focused: bool,
    active_session: Option<&str>,
) -> NotifySurface {
    let desktop = should_fire_desktop(event, prefs, window_focused, active_session);
    match (event.wants_toast, desktop) {
        (true, true) => NotifySurface::Both,
        (true, false) => NotifySurface::Toast,
        (false, true) => NotifySurface::Desktop,
        (false, false) => NotifySurface::None,
    }
}

/// The desktop half of [`should_notify`]: preference gate, baseline gate, focus gate.
pub fn should_fire_desktop(
    event: &NotificationEvent,
    prefs: &NativeNotificationPrefs,
    window_focused: bool,
    active_session: Option<&str>,
) -> bool {
    if !prefs.enabled || !prefs.kind_enabled(event.kind) {
        return false;
    }
    if event.in_baseline {
        return false;
    }
    desktop_gate(
        event.kind,
        event.session_id.as_deref(),
        event.global,
        window_focused,
        active_session,
    )
}

/// The focus/session gate, exactly Electron's `shouldFire`:
/// * global events fire whenever the user is away (`isBackgrounded`);
/// * attention kinds (approval/input) also break through for an off-screen session
///   while the window is focused;
/// * completion kinds fire only for the active session and only while away — so a
///   busy gateway (messaging, kanban, cron) cannot spam a toast per background session.
pub fn desktop_gate(
    kind: NativeNotificationKind,
    session_id: Option<&str>,
    global: bool,
    window_focused: bool,
    active_session: Option<&str>,
) -> bool {
    let backgrounded = !window_focused;

    if global {
        return backgrounded;
    }
    if kind.is_attention() {
        return backgrounded || (session_id.is_some() && session_id != active_session);
    }
    backgrounded && session_id.is_some() && session_id == active_session
}

// ── Post-connect quiet window ──────────────────────────────────────────────────

/// Length of the quiet window after a gateway socket opens
/// (`SEED_QUIET_MS` in `src/store/notify-baseline.ts`).
pub const SEED_QUIET_MS: u64 = 4_000;

/// The post-connect quiet window. A socket opening replays state that already
/// existed (parked approvals), so those replays must not raise OS notifications.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NativeNotifyBaseline {
    quiet_until_ms: u64,
}

impl NativeNotifyBaseline {
    /// Called on every gateway `open`; opens the quiet window.
    pub fn mark_open(&mut self, now_ms: u64) {
        self.quiet_until_ms = now_ms + SEED_QUIET_MS;
    }

    /// True while replayed post-connect state should not raise an OS notification.
    pub fn within(&self, now_ms: u64) -> bool {
        now_ms < self.quiet_until_ms
    }

    /// Clear the window (teardown, tests).
    pub fn reset(&mut self) {
        self.quiet_until_ms = 0;
    }
}

// ── Throttle / dedupe ──────────────────────────────────────────────────────────

/// Replayed events for the same kind+session are collapsed within this window
/// (`THROTTLE_MS` in `src/store/native-notifications.ts`).
pub const THROTTLE_MS: u64 = 1_000;

/// Self-evicting throttle: entries older than [`THROTTLE_MS`] are pruned on every
/// dispatch, so the map cannot grow.
#[derive(Debug, Default)]
pub struct NativeThrottle {
    last_fired_at: HashMap<String, u64>,
}

impl NativeThrottle {
    /// The throttle key: `kind:sessionId ?? tag ?? (global ? "global" : "")`.
    pub fn key(event: &NotificationEvent) -> String {
        let rest = event
            .session_id
            .as_deref()
            .or(event.tag.as_deref())
            .unwrap_or(if event.global { "global" } else { "" });
        format!("{}:{rest}", event.kind.as_str())
    }

    /// Returns true when the event passes (and records it); false when an identical
    /// event fired within the last [`THROTTLE_MS`].
    pub fn should_fire(&mut self, event: &NotificationEvent, now_ms: u64) -> bool {
        self.last_fired_at
            .retain(|_, at| now_ms.saturating_sub(*at) < THROTTLE_MS);
        let key = Self::key(event);
        if self.last_fired_at.contains_key(&key) {
            return false;
        }
        self.last_fired_at.insert(key, now_ms);
        true
    }
}

// ── Main-process delivery dedupe ──────────────────────────────────────────────

/// Window in which peer renderers share one OS notification (`dedupeIntervalMs`
/// in `electron/notification-ipc.ts`).
pub const DELIVERY_DEDUPE_MS: u64 = 1_000;

/// The IPC-level dedupe key: `kind:sessionId ?? tag ?? ""` (note: no `global`
/// branch here, unlike the renderer throttle).
pub fn delivery_key(event: &NotificationEvent) -> String {
    let rest = event.session_id.as_deref().or(event.tag.as_deref()).unwrap_or("");
    format!("{}:{rest}", event.kind.as_str())
}

/// Main-process dedupe: peers sharing a source window share one OS notification for
/// the same event key. The Electron original remembers the pending delivery promise
/// and drops the entry after the window; this pure port only tracks the window
/// (a duplicate never extends it).
#[derive(Debug, Default)]
pub struct DeliveryDedupe {
    last_delivery_at: HashMap<String, u64>,
}

impl DeliveryDedupe {
    /// Returns true when this key already delivered within [`DELIVERY_DEDUPE_MS`].
    pub fn is_duplicate(&mut self, key: &str, now_ms: u64) -> bool {
        self.last_delivery_at
            .retain(|_, at| now_ms.saturating_sub(*at) < DELIVERY_DEDUPE_MS);
        if self.last_delivery_at.contains_key(key) {
            return true;
        }
        self.last_delivery_at.insert(key.to_owned(), now_ms);
        false
    }
}

// ── Notification retention ─────────────────────────────────────────────────────

/// Bound on how long a live notification object is retained
/// (`NOTIFICATION_RETENTION_TTL_MS` in `electron/notification-registry.ts`).
pub const NOTIFICATION_RETENTION_TTL_MS: u64 = 10 * 60 * 1_000;

/// Events that release a retained notification (Windows may fire `close` on banner
/// timeout while Action Center keeps it clickable; Linux opts into `close` too).
pub const RELEASE_EVENTS: [&str; 3] = ["click", "action", "failed"];

// ── Linux presenter (D-Bus) ────────────────────────────────────────────────────

/// App name sent to `org.freedesktop.Notifications.Notify`.
pub const LINUX_APP_NAME: &str = "Hermes";
/// `desktop-entry` hint.
pub const LINUX_DESKTOP_ENTRY: &str = "hermes";
/// Urgency hint: 1 = normal (`notification-linux.ts`).
pub const LINUX_URGENCY_NORMAL: u8 = 1;
/// Per-call delivery timeout (`DELIVERY_TIMEOUT_MS`).
pub const LINUX_DELIVERY_TIMEOUT_MS: u64 = 5_000;
/// Cooldown after a delivery failure (`RETRY_COOLDOWN_MS`).
pub const LINUX_RETRY_COOLDOWN_MS: u64 = 10_000;
/// Maximum concurrent pending deliveries (`MAX_PENDING`).
pub const LINUX_MAX_PENDING: usize = 32;
/// Label of the built-in "open" action (`['default', 'View', …]`).
pub const LINUX_VIEW_ACTION_TEXT: &str = "View";

/// The D-Bus action array for a notification: `['default', 'View', '0', text0, …]`
/// only when the daemon advertises the `actions` capability and
/// `ELECTRON_USE_UBUNTU_NOTIFIER` is unset — otherwise empty.
pub fn linux_actions(
    caps_include_actions: bool,
    ubuntu_notifier_env_set: bool,
    action_texts: &[String],
) -> Vec<String> {
    if !caps_include_actions || ubuntu_notifier_env_set {
        return Vec::new();
    }
    let mut out = vec!["default".to_owned(), LINUX_VIEW_ACTION_TEXT.to_owned()];
    for (index, text) in action_texts.iter().enumerate() {
        out.push(index.to_string());
        out.push(text.clone());
    }
    out
}

// ── Native copy (titles/bodies) ────────────────────────────────────────────────

/// Approval notification title, unnamed (`notifications.native.approvalTitle`).
pub const APPROVAL_TITLE: &str = "Approval needed";
/// Input notification title, unnamed (`notifications.native.inputTitle`).
pub const INPUT_TITLE: &str = "Input needed";
/// Input notification body (`notifications.native.inputBody`).
pub const INPUT_BODY: &str = "Hermes is waiting for your response.";
/// Turn-finished notification title (`notifications.native.turnDoneTitle`).
pub const TURN_DONE_TITLE: &str = "Hermes finished";
/// Turn-failed notification title (`notifications.native.turnErrorTitle`).
pub const TURN_ERROR_TITLE: &str = "Turn failed";
/// Background-task finished title (`notifications.native.backgroundDoneTitle`).
pub const BACKGROUND_DONE_TITLE: &str = "Background task finished";
/// Background-task failed title (`notifications.native.backgroundFailedTitle`).
pub const BACKGROUND_FAILED_TITLE: &str = "Background task failed";
/// Credits notification title (`notifications.native.creditsTitle`).
pub const CREDITS_TITLE: &str = "Credits";
/// Approve button text (`notifications.native.approveAction`).
pub const APPROVE_ACTION_TEXT: &str = "Approve";
/// Reject button text (`notifications.native.rejectAction`).
pub const REJECT_ACTION_TEXT: &str = "Reject";
/// Longest turn-done body (`TURN_DONE_BODY_MAX` in `src/lib/turn-done-notification-body.ts`).
pub const TURN_DONE_BODY_MAX: usize = 140;
/// Fallback turn-done body when the reply text is missing.
pub const TURN_DONE_BODY_FALLBACK: &str = "Message complete.";

/// "Approval needed — <session>" when the event has a session
/// (`notifications.native.approvalTitleNamed`); parallel parked approvals stay
/// tellable apart. The bare title is the fallback for a prompt with no session.
pub fn approval_title(session: Option<&str>) -> String {
    named_title(APPROVAL_TITLE, session)
}

/// "Input needed — <session>" when the event has a session.
pub fn input_title(session: Option<&str>) -> String {
    named_title(INPUT_TITLE, session)
}

fn named_title(base: &str, session: Option<&str>) -> String {
    match session {
        Some(name) if !name.is_empty() => format!("{base} — {name}"),
        _ => base.to_owned(),
    }
}

/// Background-task title: the failed state picks [`BACKGROUND_FAILED_TITLE`].
pub const fn background_done_title(failed: bool) -> &'static str {
    if failed {
        BACKGROUND_FAILED_TITLE
    } else {
        BACKGROUND_DONE_TITLE
    }
}

/// Turn-done body: prefer the assistant reply, else `fallback` (itself falling back
/// to [`TURN_DONE_BODY_FALLBACK`]), trimmed and cut to [`TURN_DONE_BODY_MAX`]
/// characters. Never the session title.
pub fn turn_done_body(content: Option<&str>, fallback: Option<&str>) -> String {
    let reply = content.unwrap_or("").trim();
    let fallback = fallback.unwrap_or(TURN_DONE_BODY_FALLBACK).trim();
    let source = if !reply.is_empty() {
        reply
    } else if !fallback.is_empty() {
        fallback
    } else {
        TURN_DONE_BODY_FALLBACK
    };
    source.chars().take(TURN_DONE_BODY_MAX).collect()
}

/// Background-task body (`composer-status.ts`): the command title, an added
/// `" (exit N)"` when the exit code is known, and the last three output lines,
/// each on its own line.
pub fn background_done_body(title: &str, exit_code: Option<i32>, output: Option<&str>) -> String {
    let mut body = title.to_owned();
    if let Some(code) = exit_code {
        body.push_str(&format!(" (exit {code})"));
    }
    if let Some(output) = output {
        let trimmed = output.trim();
        let tail: Vec<&str> = trimmed.split('\n').collect();
        body.push('\n');
        body.push_str(&tail[tail.len().saturating_sub(3)..].join("\n"));
    }
    body
}

/// Short label for a session the renderer has no row for: the id tail still tells
/// three parked approvals apart (`shortSessionId` in the Electron store).
pub fn short_session_id(id: &str) -> String {
    let count = id.chars().count();
    let tail: String = id.chars().skip(count.saturating_sub(6)).collect();
    format!("#{tail}")
}

/// Session naming order — title → preview → short id — with the 80-character
/// ellipsis truncation (`withSessionLabel` in `src/store/native-notifications.ts`).
pub fn session_display_name(title: Option<&str>, preview: Option<&str>, session_id: &str) -> String {
    let name = title
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| preview.map(str::trim).filter(|s| !s.is_empty()))
        .map(str::to_owned)
        .unwrap_or_else(|| short_session_id(session_id));
    if name.chars().count() > 80 {
        let head: String = name.chars().take(80).collect();
        format!("{}…", head.trim_end())
    } else {
        name
    }
}

// ── Plugin notifications ───────────────────────────────────────────────────────

/// Plugin notifications always fire whenever the user is away, tagged by plugin id
/// so unrelated emitters cannot collapse into one another
/// (`dispatchPluginNativeNotification`). The title/body and any action buttons come
/// from the plugin.
pub fn plugin_notification(plugin_id: &str) -> NotificationEvent {
    NotificationEvent::native(NativeNotificationKind::Plugin)
        .global()
        .tag(plugin_id)
}

/// Mint the renderer-side notify id: `<pluginId>:<base36 time>:<6 base36 chars>`.
/// The random half is injected so the function stays pure. Callers must register
/// per-notification callbacks only when the dispatch actually fired.
pub fn mint_notify_id(plugin_id: &str, now_ms: u64, random: u64) -> String {
    let tail = to_base36(random);
    let suffix = if tail.len() > 6 {
        &tail[tail.len() - 6..]
    } else {
        tail.as_str()
    };
    format!("{plugin_id}:{}:{suffix}", to_base36(now_ms))
}

fn to_base36(mut n: u64) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".to_owned();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

/// The two credit notices that earn an OS notification
/// (`NATIVE_NOTICE_KEYS` in `src/store/agent-notices.ts`); credit-usage and
/// grant-spent notices stay in-app toasts.
pub const NATIVE_NOTICE_KEYS: [&str; 2] = ["credits.depleted", "credits.restored"];

/// Map a notice to a credits event, or `None` when it is not one of the urgent
/// credit notices. The notice body (which already carries its glyph) and the
/// caller-localized title pass through to the notification unchanged.
pub fn credits_notice_event(key: &str, text: &str) -> Option<NotificationEvent> {
    if text.trim().is_empty() || !NATIVE_NOTICE_KEYS.contains(&key) {
        return None;
    }
    Some(NotificationEvent::native(NativeNotificationKind::Credits).global())
}

// ── Test send ──────────────────────────────────────────────────────────────────

/// Settings "send test" title (`settings.notifications.testTitle`). The test
/// notification bypasses every gate so the panel can flag a silent permission
/// failure instead of looking dead.
pub const TEST_NOTIFY_TITLE: &str = "Hermes";
/// Settings "send test" body (`settings.notifications.testBody`).
pub const TEST_NOTIFY_BODY: &str = "Notifications are working.";

// ── Approval responses ─────────────────────────────────────────────────────────

/// The choice a notification button maps to (`respondToApprovalAction`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalChoice {
    /// "Approve" → `approval.respond {choice: "once"}`.
    Once,
    /// "Reject" → `approval.respond {choice: "deny"}`.
    Deny,
}

impl ApprovalChoice {
    /// The RPC choice string.
    pub const fn rpc_choice(self) -> &'static str {
        match self {
            Self::Once => "once",
            Self::Deny => "deny",
        }
    }
}

/// A parsed notification action id: `approve` / `reject`, optionally carrying the
/// exact request id (`approve:<requestId>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalActionId {
    /// Approve or reject.
    pub choice: ApprovalChoice,
    /// The request this responds to; `None` = resolve via the session's parked
    /// request (an empty `approve:` tail counts as absent).
    pub request_id: Option<String>,
}

/// Parse a notification approval action id. Unknown actions yield `None`.
pub fn parse_approval_action_id(action_id: &str) -> Option<ApprovalActionId> {
    let (action, rest) = match action_id.split_once(':') {
        Some((action, rest)) => (action, Some(rest)),
        None => (action_id, None),
    };
    let choice = match action {
        "approve" => ApprovalChoice::Once,
        "reject" => ApprovalChoice::Deny,
        _ => return None,
    };
    Some(ApprovalActionId {
        choice,
        request_id: rest.filter(|r| !r.is_empty()).map(str::to_owned),
    })
}

/// The action buttons attached to an approval notification
/// (`server-requests.ts`): Approve / Reject, each carrying the request id when known.
pub fn approval_actions(request_id: Option<&str>) -> Vec<(String, String)> {
    let suffix = request_id.map(|id| format!(":{id}")).unwrap_or_default();
    vec![
        (format!("approve{suffix}"), APPROVE_ACTION_TEXT.to_owned()),
        (format!("reject{suffix}"), REJECT_ACTION_TEXT.to_owned()),
    ]
}

// ── In-app toasts ──────────────────────────────────────────────────────────────

/// Toast severity (`NotificationKind` in `src/store/notifications.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotificationKind {
    /// Failure.
    Error,
    /// Caution.
    Warning,
    /// Neutral information (the default when none is given).
    Info,
    /// Confirmation.
    Success,
}

impl NotificationKind {
    /// Every kind.
    pub const ALL: [Self; 4] = [Self::Error, Self::Warning, Self::Info, Self::Success];

    /// Token used for storage/debug (`error`, `warning`, …).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
            Self::Success => "success",
        }
    }

    /// Sticky kinds: `defaultDuration` returns 0 for them, so they stay until
    /// dismissed.
    pub const fn is_sticky(self) -> bool {
        matches!(self, Self::Error | Self::Warning)
    }
}

/// Where a toast renders (`NotificationPlacement`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastPlacement {
    /// Top-center — the interruption slot (errors, warnings, anything with a button).
    Default,
    /// Bottom-right — ambient feedback.
    BottomRight,
}

impl ToastPlacement {
    /// Token (`default`, `bottom-right`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::BottomRight => "bottom-right",
        }
    }
}

/// Default lifetime of a non-sticky toast (`defaultDuration`: 5 s).
pub const TOAST_DEFAULT_MS: u64 = 5_000;

/// `defaultDuration`: 0 (sticky) for error/warning, [`TOAST_DEFAULT_MS`] otherwise.
pub const fn toast_default_duration_ms(kind: NotificationKind) -> u64 {
    if kind.is_sticky() { 0 } else { TOAST_DEFAULT_MS }
}

/// `defaultPlacement`: errors, warnings and anything with an action button go to
/// the top-center slot; routine confirmations stay bottom-right.
pub const fn toast_default_placement(kind: NotificationKind, has_action: bool) -> ToastPlacement {
    if kind.is_sticky() || has_action {
        ToastPlacement::Default
    } else {
        ToastPlacement::BottomRight
    }
}

/// Pluralized "N more notifications" label (`notifications.more`).
pub fn toast_more_label(count: usize) -> String {
    format!("{count} more notification{}", if count == 1 { "" } else { "s" })
}

/// Inputs for one toast (`NotificationInput` in `src/store/notifications.ts`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToastInput {
    /// Stable id: a repeat with the same id replaces the previous toast (and
    /// restarts its timer) instead of stacking.
    pub id: Option<String>,
    /// Severity; defaults to [`NotificationKind::Info`].
    pub kind: Option<NotificationKind>,
    /// Secondary detail line, muted.
    pub meta: Option<String>,
    /// Bold title above the message.
    pub title: Option<String>,
    /// The message itself.
    pub message: String,
    /// Dimmed technical detail below the message.
    pub detail: Option<String>,
    /// Primary action button label.
    pub action_label: Option<String>,
    /// Quieter second button beside the primary one.
    pub secondary_action_label: Option<String>,
    /// Override the kind's default lifetime (milliseconds; 0 = sticky).
    pub duration_ms: Option<u64>,
    /// Override the computed placement.
    pub placement: Option<ToastPlacement>,
}

/// A shown toast as the feed holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    /// Stable id.
    pub id: String,
    /// Severity.
    pub kind: NotificationKind,
    /// Bold title above the message.
    pub title: Option<String>,
    /// The message.
    pub message: String,
    /// Dimmed technical detail below the message.
    pub detail: Option<String>,
    /// Secondary detail line, muted.
    pub meta: Option<String>,
    /// Where it renders.
    pub placement: ToastPlacement,
    /// When it appeared (ms since the Unix epoch).
    pub created_at_ms: u64,
    /// How long it lives; 0 = sticky until dismissed.
    pub duration_ms: u64,
    /// Primary action button label.
    pub action_label: Option<String>,
    /// Secondary action button label.
    pub secondary_action_label: Option<String>,
}

impl Toast {
    /// Build a toast the way `showNotification` does: kind defaults to info,
    /// duration and placement default per kind (and presence of an action), and the
    /// id falls back to `<now>-<seq>` using a caller-supplied sequence.
    pub fn from_input(input: ToastInput, now_ms: u64, seq: u64) -> Self {
        let kind = input.kind.unwrap_or(NotificationKind::Info);
        let has_action = input.action_label.is_some();
        Self {
            id: input.id.unwrap_or_else(|| format!("{now_ms}-{seq}")),
            kind,
            title: input.title,
            message: input.message,
            detail: input.detail,
            meta: input.meta,
            placement: input
                .placement
                .unwrap_or_else(|| toast_default_placement(kind, has_action)),
            created_at_ms: now_ms,
            duration_ms: input.duration_ms.unwrap_or_else(|| toast_default_duration_ms(kind)),
            action_label: input.action_label,
            secondary_action_label: input.secondary_action_label,
        }
    }

    /// When this toast expires, or `None` when sticky.
    pub fn expires_at_ms(&self) -> Option<u64> {
        if self.duration_ms == 0 {
            None
        } else {
            Some(self.created_at_ms + self.duration_ms)
        }
    }
}

/// The in-app toast feed: newest first, replace-by-id (a repeat with the same id
/// moves to the front and restarts its timer), no length cap — visual depth is
/// capped by the view (`CardStack`), not by discarding queued toasts.
#[derive(Debug, Default)]
pub struct ToastFeed {
    items: Vec<Toast>,
}

impl ToastFeed {
    /// An empty feed.
    pub fn new() -> Self {
        Self::default()
    }

    /// Show a toast. A repeat with an existing id replaces it in place (at the
    /// front) and restarts its lifetime.
    pub fn push(&mut self, toast: Toast) {
        self.items.retain(|item| item.id != toast.id);
        self.items.insert(0, toast);
    }

    /// Dismiss one toast; returns it so the caller can run its dismiss callback.
    pub fn dismiss(&mut self, id: &str) -> Option<Toast> {
        let position = self.items.iter().position(|item| item.id == id)?;
        Some(self.items.remove(position))
    }

    /// Clear the feed; returns every removed toast (callers run their dismiss
    /// callbacks for all of them).
    pub fn clear(&mut self) -> Vec<Toast> {
        std::mem::take(&mut self.items)
    }

    /// Ids whose (non-zero) lifetime has elapsed at `now_ms`; the caller dismisses
    /// them. Sticky toasts never appear here.
    pub fn due_ids(&self, now_ms: u64) -> Vec<String> {
        self.items
            .iter()
            .filter(|item| item.expires_at_ms().is_some_and(|at| now_ms >= at))
            .map(|item| item.id.clone())
            .collect()
    }

    /// The toasts, newest first.
    pub fn items(&self) -> &[Toast] {
        &self.items
    }

    /// How many toasts render when the stack is collapsed (front card only) or
    /// expanded (all of them).
    pub fn visible_len(&self, expanded: bool) -> usize {
        if expanded {
            self.items.len()
        } else {
            self.items.len().min(1)
        }
    }

    /// How many toasts are folded behind the front card (`toast_more_label`).
    pub fn hidden_len(&self, expanded: bool) -> usize {
        self.items.len() - self.visible_len(expanded)
    }
}

// ── Error summarising ──────────────────────────────────────────────────────────

/// The toast's error category (`ErrorToastCategory` in `src/store/notifications.ts`).
/// [`readable_error`] never returns `Timeout` itself: callers flip `Unclassified`
/// to `Timeout` when their error is a timeout, mirroring the Electron `notifyError`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorToastCategory {
    /// No rule matched.
    Unclassified,
    /// Caller-detected timeout (never produced by [`readable_error`]).
    Timeout,
    /// Disk full / ENOSPC / SQLITE_FULL.
    DiskFull,
    /// Another classified storage failure (locked, corrupt, read-only …).
    StorageFailure,
    /// The gateway no longer accepts the saved sign-in.
    GatewayAuthFailed,
    /// A provider rejected an API key.
    ApiKeyRejected,
    /// An API key is missing.
    ApiKeyMissing,
    /// The backend rejected the method (skew after an update).
    MethodNotAllowed,
    /// Microphone permission denied.
    MicrophonePermission,
    /// App and backend out of sync (different versions).
    RpcOutOfSync,
    /// The backend needs a restart to finish an update.
    RestartRequired,
}

impl ErrorToastCategory {
    /// Token used for telemetry (`unclassified`, `disk_full`, …).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unclassified => "unclassified",
            Self::Timeout => "timeout",
            Self::DiskFull => "disk_full",
            Self::StorageFailure => "storage_failure",
            Self::GatewayAuthFailed => "gateway_auth_failed",
            Self::ApiKeyRejected => "api_key_rejected",
            Self::ApiKeyMissing => "api_key_missing",
            Self::MethodNotAllowed => "method_not_allowed",
            Self::MicrophonePermission => "microphone_permission",
            Self::RpcOutOfSync => "rpc_out_of_sync",
            Self::RestartRequired => "restart_required",
        }
    }
}

/// A one-click recovery attached to a toast (`RECOVERY_ACTIONS`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryAction {
    /// Open the Updates window.
    OpenUpdates,
    /// Restart the backend.
    RestartHermes,
    /// Open Settings → Keys with `env_key` highlighted.
    OpenKeys {
        /// The environment-variable key to deep-link to.
        env_key: &'static str,
    },
    /// Open Settings → Gateways.
    OpenGateways,
    /// Open Command Center → Maintenance.
    OpenMaintenance,
}

impl RecoveryAction {
    /// Button label (`notifications.actions.*` + `notifications.updateHermes`).
    pub const fn label(&self) -> &'static str {
        match self {
            Self::OpenUpdates => "Update Hermes",
            Self::RestartHermes => "Restart Hermes",
            Self::OpenKeys { .. } => "Open Keys",
            Self::OpenGateways => "Open Gateways",
            Self::OpenMaintenance => "Open Maintenance",
        }
    }

    /// Hash-router route to open, when this action navigates. `RestartHermes` and
    /// `OpenUpdates` have no route (they restart the backend / open a window).
    pub fn route(&self) -> Option<String> {
        match self {
            Self::RestartHermes | Self::OpenUpdates => None,
            Self::OpenKeys { env_key } => Some(keys_route(env_key)),
            Self::OpenGateways => Some(GATEWAY_SETTINGS_ROUTE.to_owned()),
            Self::OpenMaintenance => Some(MAINTENANCE_ROUTE.to_owned()),
        }
    }
}

/// Settings → Keys deep link with the key highlighted (`KEYS_ROUTE`).
pub const KEYS_ROUTE_PREFIX: &str = "/settings?tab=keys&key=";
/// Settings → Gateways route (`GATEWAY_SETTINGS_ROUTE`).
pub const GATEWAY_SETTINGS_ROUTE: &str = "/settings?tab=gateway";
/// Command Center → Maintenance route (`MAINTENANCE_ROUTE`).
pub const MAINTENANCE_ROUTE: &str = "/command-center?section=maintenance";

/// `/settings?tab=keys&key=<envKey>` with `envKey` percent-encoded.
pub fn keys_route(env_key: &str) -> String {
    format!("{KEYS_ROUTE_PREFIX}{}", percent_encode(env_key))
}

/// Readable error produced by [`readable_error`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadableError {
    /// The summarised, user-facing message.
    pub message: String,
    /// The original (cleaned) text when it adds information beyond the message.
    pub detail: Option<String>,
    /// Recovery button, when a rule provides one.
    pub action: Option<RecoveryAction>,
    /// Category for telemetry.
    pub category: ErrorToastCategory,
}

/// Unwrap the Electron IPC bridge wrapper
/// (`Error invoking remote method '<x>': Error: <rest>`).
fn unwrap_ipc_error(raw: &str) -> &str {
    const MARKER: &str = "Error invoking remote method '";
    if let Some(start) = raw.find(MARKER) {
        let rest = &raw[start + MARKER.len()..];
        if let Some(end) = rest.find("': Error: ") {
            return &rest[end + "': Error: ".len()..];
        }
    }
    raw
}

/// `cleanErrorText`: strip one leading `Error:` (plus whitespace) and trim.
fn clean_error_text(value: &str) -> String {
    let stripped = value.strip_prefix("Error:").map(str::trim_start).unwrap_or(value);
    stripped.trim().to_owned()
}

/// `"detail"` extraction: the first `"detail": "…"` value, else `None`.
fn extract_detail(cleaned: &str) -> Option<String> {
    for marker in ["\"detail\"", "'detail'"] {
        if let Some(start) = cleaned.find(marker) {
            let rest = &cleaned[start + marker.len()..];
            let rest = rest.trim_start();
            let rest = rest.strip_prefix(':').unwrap_or(rest).trim_start();
            if let Some(rest) = rest.strip_prefix('"')
                && let Some(end) = rest.find('"')
            {
                return Some(rest[..end].to_owned());
            }
        }
    }
    None
}

fn contains_ci(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

/// True when an error string is a disk-full / ENOSPC / SQLITE_FULL failure
/// (`isDiskFullErrorMessage`).
pub fn is_disk_full_message(message: &str) -> bool {
    [
        "no space left on device",
        "not enough space",
        "database or disk is full",
        "enospc",
        "disk full",
        "full disk",
    ]
    .iter()
    .any(|needle| contains_ci(message, needle))
}

/// The quoted value after a `"code"` / `'code'` key, e.g. `storage_write_failed`
/// in `{"code": "storage_write_failed"}`.
pub fn extract_error_code(message: &str) -> Option<String> {
    for marker in ["\"code\"", "'code'"] {
        let mut from = 0usize;
        while let Some(offset) = message[from..].find(marker) {
            let start = from + offset + marker.len();
            let rest = message[start..].trim_start();
            let rest = rest.strip_prefix(':').map(str::trim_start).unwrap_or(rest);
            if let Some(quote) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') {
                let value = &rest[quote.len_utf8()..];
                if let Some(end) = value.find(quote) {
                    return Some(value[..end].to_owned());
                }
            }
            from = start;
        }
    }
    None
}

/// The summary rules in `ERROR_SUMMARIES` order. The first match wins; the strings
/// are the shipped English copy (`src/i18n/en.ts`, `notifications.errors.*`).
fn summarize_error_message(message: &str, fallback: &str) -> (String, Option<RecoveryAction>, ErrorToastCategory) {
    let code = extract_error_code(message);
    let code_is = |want: &str| code.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(want));
    let code_is_storage = code
        .as_deref()
        .is_some_and(|c| c.eq_ignore_ascii_case("disk_full") || c.to_ascii_lowercase().starts_with("storage_"));

    if is_disk_full_message(message) {
        return (
            "Disk full — free some space, then try again.".to_owned(),
            Some(RecoveryAction::OpenMaintenance),
            ErrorToastCategory::DiskFull,
        );
    }
    if code_is_storage {
        return (
            "Hermes couldn't save to its data folder. Open Maintenance to check and repair it.".to_owned(),
            Some(RecoveryAction::OpenMaintenance),
            ErrorToastCategory::StorageFailure,
        );
    }
    if code_is("gateway_auth_failed") {
        return (
            "This Hermes no longer accepts your saved sign-in. Open Gateways and sign in again (or paste a new access token), then retry.".to_owned(),
            Some(RecoveryAction::OpenGateways),
            ErrorToastCategory::GatewayAuthFailed,
        );
    }
    if contains_ci(message, "incorrect api key provided") || code_is("invalid_api_key") {
        return (
            "OpenAI didn't accept your API key. Update it in Settings → Keys, then try again.".to_owned(),
            Some(RecoveryAction::OpenKeys {
                env_key: "OPENAI_API_KEY",
            }),
            ErrorToastCategory::ApiKeyRejected,
        );
    }
    if contains_ci(message, "neither voice_tools_openai_key nor openai_api_key is set") {
        return (
            "Voice needs an OpenAI key. Add one in Settings → Keys.".to_owned(),
            Some(RecoveryAction::OpenKeys {
                env_key: "OPENAI_API_KEY",
            }),
            ErrorToastCategory::ApiKeyMissing,
        );
    }
    if message.contains("ELEVENLABS_API_KEY not set") {
        return (
            "Voice input needs an ElevenLabs key. Add one in Settings → Keys.".to_owned(),
            Some(RecoveryAction::OpenKeys {
                env_key: "ELEVENLABS_API_KEY",
            }),
            ErrorToastCategory::ApiKeyMissing,
        );
    }
    if message.contains("ElevenLabs STT API error (HTTP 401)") {
        return (
            "ElevenLabs didn't accept your API key. Update it in Settings → Keys, then try again.".to_owned(),
            Some(RecoveryAction::OpenKeys {
                env_key: "ELEVENLABS_API_KEY",
            }),
            ErrorToastCategory::ApiKeyRejected,
        );
    }
    if contains_ci(message, "method not allowed") {
        return (
            "Hermes' background service is out of step with the app, probably after an update. Restart it to fix this."
                .to_owned(),
            Some(RecoveryAction::RestartHermes),
            ErrorToastCategory::MethodNotAllowed,
        );
    }
    if contains_ci(message, "microphone permission") {
        return (
            "Microphone permission was denied.".to_owned(),
            None,
            ErrorToastCategory::MicrophonePermission,
        );
    }
    if contains_ci(message, "out of sync (different versions)") {
        return (
            "The app and the backend are on different versions. Update both.".to_owned(),
            Some(RecoveryAction::OpenUpdates),
            ErrorToastCategory::RpcOutOfSync,
        );
    }
    if contains_ci(message, "restart required:") {
        return (
            "Hermes was updated but is still running the old version. Restart it to finish the update.".to_owned(),
            Some(RecoveryAction::RestartHermes),
            ErrorToastCategory::RestartRequired,
        );
    }

    let message_out = if message.chars().count() > 180 || message.is_empty() {
        fallback.to_owned()
    } else {
        message.to_owned()
    };
    (message_out, None, ErrorToastCategory::Unclassified)
}

/// `readableError`: unwrap the IPC wrapper, strip a leading `Error:`, prefer the
/// embedded `"detail"`, apply the summary rules, and fall back to `fallback` for
/// unclassified messages longer than 180 characters (or empty).
pub fn readable_error(raw: &str, fallback: &str) -> ReadableError {
    let unwrapped = unwrap_ipc_error(raw);
    let cleaned = clean_error_text(unwrapped);
    let detail = extract_detail(&cleaned).unwrap_or_else(|| cleaned.clone());
    let (message, action, category) = summarize_error_message(&detail, fallback);
    let detail_out = if detail == message || detail.is_empty() {
        None
    } else {
        Some(detail)
    };
    ReadableError {
        message,
        detail: detail_out,
        action,
        category,
    }
}

/// Minimal `encodeURIComponent` equivalent for route query values.
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

// ── Settings-page identity ─────────────────────────────────────────────────────

/// Settings tab that holds the notification rows (`settings?tab=notifications`).
pub const NOTIFICATIONS_SETTINGS_TAB: &str = "notifications";
/// Subpage holding the alert toggles.
pub const NOTIFICATIONS_SUBPAGE_ALERTS: &str = "alerts";
/// Subpage holding the completion-sound picker.
pub const NOTIFICATIONS_SUBPAGE_SOUNDS: &str = "sounds";
/// Deep-link id of the master switch (`SETTING_IDS.notifications.enableAll`).
pub const ENABLE_ALL_SETTING_ID: &str = "notifications.enable-all";

#[cfg(test)]
mod tests {
    use super::*;

    fn prefs_all_on() -> NativeNotificationPrefs {
        NativeNotificationPrefs::default()
    }

    // ── Settings ──────────────────────────────────────────────────────────────

    #[test]
    fn prefs_default_to_all_on() {
        let prefs = prefs_all_on();
        assert!(prefs.enabled);
        for kind in NativeNotificationKind::ALL {
            assert!(prefs.kind_enabled(kind), "{kind} should default on");
        }
    }

    #[test]
    fn prefs_decode_partial_and_corrupt_storage() {
        let decoded = NativeNotificationPrefs::from_stored_json(
            r#"{"enabled": false, "kinds": {"approval": false, "nonsense": false}}"#,
        );
        assert!(!decoded.enabled);
        assert!(!decoded.kind_enabled(NativeNotificationKind::Approval));
        assert!(decoded.kind_enabled(NativeNotificationKind::Input));

        assert_eq!(
            NativeNotificationPrefs::from_stored_json("not json"),
            NativeNotificationPrefs::default()
        );
        // Non-boolean values are ignored, like Electron's typeof check.
        let lenient = NativeNotificationPrefs::from_stored_json(r#"{"enabled": "yes", "kinds": {"approval": 1}}"#);
        assert_eq!(lenient, NativeNotificationPrefs::default());
    }

    #[test]
    fn prefs_round_trip_through_json() {
        let mut prefs = prefs_all_on();
        prefs.set_enabled(false);
        prefs.set_kind(NativeNotificationKind::Credits, false);
        let decoded = NativeNotificationPrefs::from_stored_json(&prefs.to_stored_json());
        assert_eq!(decoded, prefs);
    }

    // ── The decision (mirrors native-notifications.test.ts) ───────────────────

    #[test]
    fn completion_fires_for_active_session_when_away() {
        let event = NotificationEvent::native(NativeNotificationKind::TurnDone).session("s1");
        assert_eq!(
            should_notify(&event, &prefs_all_on(), false, Some("s1")),
            NotifySurface::Desktop
        );
    }

    #[test]
    fn completion_suppressed_when_focused_and_for_other_sessions() {
        let event = NotificationEvent::native(NativeNotificationKind::TurnDone).session("s1");
        // Focused window: no notification for the active session.
        assert_eq!(
            should_notify(&event, &prefs_all_on(), true, Some("s1")),
            NotifySurface::None
        );
        // Unfocused but a different session: no gateway spam.
        assert_eq!(
            should_notify(&event, &prefs_all_on(), false, Some("s2")),
            NotifySurface::None
        );
    }

    #[test]
    fn attention_fires_for_offscreen_session_even_while_focused() {
        let event = NotificationEvent::native(NativeNotificationKind::Approval).session("s1");
        assert_eq!(
            should_notify(&event, &prefs_all_on(), true, Some("s2")),
            NotifySurface::Desktop
        );
        // ... but not for the active session while focused.
        assert_eq!(
            should_notify(&event, &prefs_all_on(), true, Some("s1")),
            NotifySurface::None
        );
    }

    #[test]
    fn global_fires_only_while_away_with_no_session_requirement() {
        let event = NotificationEvent::native(NativeNotificationKind::Credits).global();
        assert_eq!(
            should_notify(&event, &prefs_all_on(), false, None),
            NotifySurface::Desktop
        );
        assert_eq!(should_notify(&event, &prefs_all_on(), true, None), NotifySurface::None);
    }

    #[test]
    fn settings_gate_the_desktop_half_only() {
        let mut prefs = prefs_all_on();
        prefs.set_enabled(false);
        let event = NotificationEvent::native(NativeNotificationKind::TurnDone)
            .session("s1")
            .toast();
        assert_eq!(should_notify(&event, &prefs, false, Some("s1")), NotifySurface::Toast);

        let mut prefs = prefs_all_on();
        prefs.set_kind(NativeNotificationKind::TurnDone, false);
        assert_eq!(should_notify(&event, &prefs, false, Some("s1")), NotifySurface::Toast);
        assert_eq!(
            should_notify(&event, &prefs_all_on(), false, Some("s1")),
            NotifySurface::Both
        );
    }

    #[test]
    fn baseline_suppresses_the_desktop_half_only() {
        let event = NotificationEvent::native(NativeNotificationKind::Approval)
            .session("s1")
            .toast()
            .in_baseline(true);
        assert_eq!(
            should_notify(&event, &prefs_all_on(), false, None),
            NotifySurface::Toast
        );
        assert!(!should_fire_desktop(&event, &prefs_all_on(), false, None));
    }

    #[test]
    fn baseline_window_is_four_seconds() {
        let mut baseline = NativeNotifyBaseline::default();
        baseline.mark_open(1_000);
        assert!(baseline.within(4_999));
        assert!(!baseline.within(5_000));
        baseline.reset();
        assert!(!baseline.within(4_999));
    }

    // ── Throttle / dedupe ─────────────────────────────────────────────────────

    #[test]
    fn throttle_collapses_same_kind_and_session_within_a_second() {
        let mut throttle = NativeThrottle::default();
        let event = NotificationEvent::native(NativeNotificationKind::TurnDone).session("s1");
        assert!(throttle.should_fire(&event, 0));
        assert!(!throttle.should_fire(&event, 999));
        assert!(throttle.should_fire(&event, 1_000));

        let other = NotificationEvent::native(NativeNotificationKind::TurnDone).session("s2");
        assert!(throttle.should_fire(&other, 1_000));
    }

    #[test]
    fn throttle_keys_include_tag_and_global() {
        let mut throttle = NativeThrottle::default();
        let alpha = plugin_notification("alpha");
        let beta = plugin_notification("beta");
        assert!(throttle.should_fire(&alpha, 0));
        assert!(throttle.should_fire(&beta, 0));
        assert!(!throttle.should_fire(&alpha, 100));

        let global = NotificationEvent::native(NativeNotificationKind::Credits).global();
        assert_eq!(NativeThrottle::key(&global), "credits:global");
        assert_eq!(NativeThrottle::key(&alpha), "plugin:alpha");
        let session_wins = plugin_notification("alpha").session("s9");
        assert_eq!(NativeThrottle::key(&session_wins), "plugin:s9");
    }

    #[test]
    fn delivery_dedupe_uses_ipc_key_and_window() {
        let mut dedupe = DeliveryDedupe::default();
        let event = NotificationEvent::native(NativeNotificationKind::TurnDone)
            .session("s1")
            .tag("ignored");
        assert_eq!(delivery_key(&event), "turnDone:s1");
        assert!(!dedupe.is_duplicate("turnDone:s1", 0));
        assert!(dedupe.is_duplicate("turnDone:s1", 10));
        assert!(!dedupe.is_duplicate("turnDone:s1", 1_000));

        let tag_only = plugin_notification("alpha");
        assert_eq!(delivery_key(&tag_only), "plugin:alpha");
    }

    // ── Linux presenter ───────────────────────────────────────────────────────

    #[test]
    fn linux_actions_follow_capability_and_env() {
        let texts = vec![APPROVE_ACTION_TEXT.to_owned(), REJECT_ACTION_TEXT.to_owned()];
        assert_eq!(
            linux_actions(true, false, &texts),
            vec!["default", "View", "0", "Approve", "1", "Reject"]
        );
        assert!(linux_actions(false, false, &texts).is_empty());
        assert!(linux_actions(true, true, &texts).is_empty());
    }

    // ── Copy ──────────────────────────────────────────────────────────────────

    #[test]
    fn named_titles_append_the_session() {
        assert_eq!(approval_title(None), "Approval needed");
        assert_eq!(
            approval_title(Some("Fix the flaky test")),
            "Approval needed — Fix the flaky test"
        );
        assert_eq!(input_title(Some("Deploy")), "Input needed — Deploy");
    }

    #[test]
    fn turn_done_body_prefers_reply_trims_and_caps_at_140() {
        assert_eq!(turn_done_body(Some("  hello  "), None), "hello");
        assert_eq!(turn_done_body(Some(""), Some("done.")), "done.");
        assert_eq!(turn_done_body(None, Some("  ")), "Message complete.");

        let long = "x".repeat(200);
        assert_eq!(turn_done_body(Some(&long), None), "x".repeat(140));
    }

    #[test]
    fn background_done_body_appends_exit_and_output_tail() {
        assert_eq!(background_done_body("npm test", None, None), "npm test");
        assert_eq!(background_done_body("npm test", Some(1), None), "npm test (exit 1)");
        assert_eq!(
            background_done_body("npm test", Some(0), Some("a\nb\nc\nd\n")),
            "npm test (exit 0)\nb\nc\nd"
        );
    }

    #[test]
    fn session_labels_prefer_title_then_preview_then_id_tail() {
        assert_eq!(
            session_display_name(Some(" Title "), Some("preview"), "abcdef123456"),
            "Title"
        );
        assert_eq!(session_display_name(None, Some("preview"), "abcdef123456"), "preview");
        assert_eq!(session_display_name(None, None, "abcdef123456"), "#123456");
    }

    #[test]
    fn session_labels_truncate_past_80_chars() {
        let long = format!("{}   ", "n".repeat(90));
        let labelled = session_display_name(Some(&long), None, "s");
        assert_eq!(labelled.chars().count(), 81); // 80 chars + the ellipsis
        assert!(labelled.ends_with('…'));
    }

    #[test]
    fn approval_actions_carry_the_request_id() {
        assert_eq!(
            approval_actions(Some("req-1")),
            vec![
                ("approve:req-1".to_owned(), "Approve".to_owned()),
                ("reject:req-1".to_owned(), "Reject".to_owned())
            ]
        );
        assert_eq!(approval_actions(None)[0].0, "approve");
    }

    #[test]
    fn approval_action_ids_parse_like_respond_to_approval_action() {
        let approve = parse_approval_action_id("approve:req-1").unwrap();
        assert_eq!(approve.choice, ApprovalChoice::Once);
        assert_eq!(approve.choice.rpc_choice(), "once");
        assert_eq!(approve.request_id.as_deref(), Some("req-1"));

        let reject = parse_approval_action_id("reject").unwrap();
        assert_eq!(reject.choice, ApprovalChoice::Deny);
        assert_eq!(reject.choice.rpc_choice(), "deny");
        assert_eq!(reject.request_id, None);

        let joined = parse_approval_action_id("approve:a:b").unwrap();
        assert_eq!(joined.request_id.as_deref(), Some("a:b"));

        assert!(parse_approval_action_id("approve:").unwrap().request_id.is_none());
        assert!(parse_approval_action_id("wat").is_none());
    }

    #[test]
    fn plugin_notify_ids_mint_base36() {
        let id = mint_notify_id("pet", 1_000_000, 123_456);
        assert!(id.starts_with("pet:"));
        assert_eq!(id.split(':').count(), 3);
    }

    #[test]
    fn credits_notices_require_an_urgent_key_and_text() {
        assert!(credits_notice_event("credits.usage", "x").is_none());
        assert!(credits_notice_event("credits.depleted", "  ").is_none());
        let event = credits_notice_event("credits.depleted", " credits out ").unwrap();
        assert_eq!(event.kind, NativeNotificationKind::Credits);
        assert!(event.global);
        // A credits notice is account-wide: fires while away with no active session.
        assert_eq!(
            should_notify(&event, &prefs_all_on(), false, None),
            NotifySurface::Desktop
        );
    }

    // ── Toasts ────────────────────────────────────────────────────────────────

    #[test]
    fn toast_defaults_match_notifications_ts() {
        assert_eq!(toast_default_duration_ms(NotificationKind::Error), 0);
        assert_eq!(toast_default_duration_ms(NotificationKind::Warning), 0);
        assert_eq!(toast_default_duration_ms(NotificationKind::Info), 5_000);
        assert_eq!(toast_default_duration_ms(NotificationKind::Success), 5_000);

        assert_eq!(
            toast_default_placement(NotificationKind::Info, false),
            ToastPlacement::BottomRight
        );
        assert_eq!(
            toast_default_placement(NotificationKind::Error, false),
            ToastPlacement::Default
        );
        assert_eq!(
            toast_default_placement(NotificationKind::Success, true),
            ToastPlacement::Default
        );
    }

    #[test]
    fn toast_from_input_fills_defaults_and_generates_ids() {
        let toast = Toast::from_input(
            ToastInput {
                message: "saved".to_owned(),
                ..Default::default()
            },
            42,
            7,
        );
        assert_eq!(toast.id, "42-7");
        assert_eq!(toast.kind, NotificationKind::Info);
        assert_eq!(toast.duration_ms, 5_000);
        assert_eq!(toast.placement, ToastPlacement::BottomRight);
        assert_eq!(toast.expires_at_ms(), Some(5_042));
    }

    #[test]
    fn feed_replaces_by_id_keeps_newest_first_and_restarts_the_timer() {
        let mut feed = ToastFeed::new();
        let first = Toast::from_input(
            ToastInput {
                id: Some("turn-error".to_owned()),
                kind: Some(NotificationKind::Error),
                message: "one".to_owned(),
                ..Default::default()
            },
            0,
            0,
        );
        let second = Toast::from_input(
            ToastInput {
                message: "other".to_owned(),
                ..Default::default()
            },
            1,
            1,
        );
        feed.push(first);
        feed.push(second);
        assert_eq!(feed.items()[0].message, "other");

        let repeat = Toast::from_input(
            ToastInput {
                id: Some("turn-error".to_owned()),
                kind: Some(NotificationKind::Error),
                message: "one (again)".to_owned(),
                ..Default::default()
            },
            10,
            2,
        );
        feed.push(repeat);
        assert_eq!(feed.items().len(), 2);
        assert_eq!(feed.items()[0].id, "turn-error");
        assert!(feed.due_ids(10).is_empty()); // sticky error: never due
    }

    #[test]
    fn feed_lifetimes_and_clear() {
        let mut feed = ToastFeed::new();
        feed.push(Toast::from_input(
            ToastInput {
                message: "info".to_owned(),
                ..Default::default()
            },
            0,
            0,
        ));
        feed.push(Toast::from_input(
            ToastInput {
                kind: Some(NotificationKind::Error),
                message: "sticky".to_owned(),
                ..Default::default()
            },
            0,
            1,
        ));

        assert_eq!(feed.due_ids(4_999), Vec::<String>::new());
        assert_eq!(feed.due_ids(5_000), vec!["0-0".to_owned()]);
        assert_eq!(feed.dismiss("0-0").map(|t| t.message), Some("info".to_owned()));
        assert!(feed.dismiss("0-0").is_none());

        let cleared = feed.clear();
        assert_eq!(cleared.len(), 1);
        assert!(feed.items().is_empty());
    }

    #[test]
    fn feed_depth_is_visual_not_stored() {
        let mut feed = ToastFeed::new();
        for i in 0..4u64 {
            feed.push(Toast::from_input(
                ToastInput {
                    message: format!("m{i}"),
                    ..Default::default()
                },
                i,
                i,
            ));
        }
        assert_eq!(feed.items().len(), 4);
        assert_eq!(feed.visible_len(false), 1);
        assert_eq!(feed.hidden_len(false), 3);
        assert_eq!(feed.visible_len(true), 4);
        assert_eq!(toast_more_label(3), "3 more notifications");
        assert_eq!(toast_more_label(1), "1 more notification");
    }

    // ── Error summarising ─────────────────────────────────────────────────────

    #[test]
    fn disk_full_rule_wins_first() {
        let summary = readable_error("sqlite3.OperationalError: database or disk is full", "Send failed");
        assert_eq!(summary.category, ErrorToastCategory::DiskFull);
        assert_eq!(summary.action, Some(RecoveryAction::OpenMaintenance));
        assert_eq!(summary.action.unwrap().route().as_deref(), Some(MAINTENANCE_ROUTE));
        assert_eq!(summary.message, "Disk full — free some space, then try again.");
        assert!(is_disk_full_message("write error: ENOSPC"));
    }

    #[test]
    fn storage_codes_map_to_maintenance() {
        let summary = readable_error(r#"{"code":"storage_write_failed"}"#, "Save failed");
        assert_eq!(summary.category, ErrorToastCategory::StorageFailure);
        assert_eq!(summary.action, Some(RecoveryAction::OpenMaintenance));
        assert_eq!(
            extract_error_code(r#"{'code': 'disk_full'}"#).as_deref(),
            Some("disk_full")
        );
    }

    #[test]
    fn auth_and_key_rules() {
        let gateway = readable_error(r#"{"code":"gateway_auth_failed"}"#, "Offline");
        assert_eq!(gateway.category, ErrorToastCategory::GatewayAuthFailed);
        assert_eq!(gateway.action.unwrap().route().as_deref(), Some(GATEWAY_SETTINGS_ROUTE));

        let rejected = readable_error("Incorrect API key provided: sk-***", "Chat failed");
        assert_eq!(rejected.category, ErrorToastCategory::ApiKeyRejected);
        assert_eq!(
            rejected.action,
            Some(RecoveryAction::OpenKeys {
                env_key: "OPENAI_API_KEY"
            })
        );
        assert_eq!(
            rejected.action.unwrap().route().as_deref(),
            Some("/settings?tab=keys&key=OPENAI_API_KEY")
        );

        let missing = readable_error(
            "neither voice_tools_openai_key nor openai_api_key is set",
            "Voice failed",
        );
        assert_eq!(missing.category, ErrorToastCategory::ApiKeyMissing);

        let eleven = readable_error("ElevenLabs STT API error (HTTP 401)", "Voice failed");
        assert_eq!(eleven.category, ErrorToastCategory::ApiKeyRejected);
        assert_eq!(
            eleven.action,
            Some(RecoveryAction::OpenKeys {
                env_key: "ELEVENLABS_API_KEY"
            })
        );

        let eleven_missing = readable_error("ELEVENLABS_API_KEY not set", "Voice failed");
        assert_eq!(eleven_missing.category, ErrorToastCategory::ApiKeyMissing);
    }

    #[test]
    fn skew_and_permission_rules() {
        let method = readable_error("Method not allowed", "Request failed");
        assert_eq!(method.category, ErrorToastCategory::MethodNotAllowed);
        assert_eq!(method.action, Some(RecoveryAction::RestartHermes));
        assert_eq!(method.action.unwrap().route(), None);

        let microphone = readable_error("Microphone permission was denied", "Voice failed");
        assert_eq!(microphone.category, ErrorToastCategory::MicrophonePermission);
        assert_eq!(microphone.action, None);

        let out_of_sync = readable_error("RPC params out of sync (different versions)", "Call failed");
        assert_eq!(out_of_sync.category, ErrorToastCategory::RpcOutOfSync);
        assert_eq!(out_of_sync.action, Some(RecoveryAction::OpenUpdates));

        let restart = readable_error("Restart required: backend is stale", "Call failed");
        assert_eq!(restart.category, ErrorToastCategory::RestartRequired);
    }

    #[test]
    fn ipc_wrapping_cleaning_and_detail_extraction() {
        let summary = readable_error("Error invoking remote method 'hermes:api': Error: boom", "fallback");
        assert_eq!(summary.message, "boom");
        assert_eq!(summary.category, ErrorToastCategory::Unclassified);
        assert_eq!(summary.action, None);
        assert_eq!(summary.detail, None);

        // The extracted "detail" replaces the message (Electron feeds the detail
        // into the summary directly), so a bare detail never repeats itself.
        let with_detail = readable_error(r#"Error: call failed {"detail":"boom detail"}"#, "fb");
        assert_eq!(with_detail.message, "boom detail");
        assert_eq!(with_detail.detail, None);
        assert_eq!(with_detail.category, ErrorToastCategory::Unclassified);

        // When a rule rewrites the message, the raw detail is kept beneath it.
        let rewritten = readable_error(r#"Error: {"detail":"Restart required: stale"}"#, "fb");
        assert_eq!(rewritten.category, ErrorToastCategory::RestartRequired);
        assert_eq!(rewritten.detail.as_deref(), Some("Restart required: stale"));
    }

    #[test]
    fn unclassified_fallbacks() {
        assert_eq!(readable_error("", "Send failed").message, "Send failed");
        let long = "e".repeat(200);
        assert_eq!(readable_error(&long, "Send failed").message, "Send failed");
        assert_eq!(readable_error("short", "Send failed").message, "short");
    }

    #[test]
    fn settings_ids_match_manifest() {
        assert_eq!(ENABLE_ALL_SETTING_ID, "notifications.enable-all");
        assert_eq!(
            NativeNotificationKind::Approval.setting_id(),
            "notifications.kind-approval"
        );
        assert_eq!(NativeNotificationKind::Plugin.setting_id(), "notifications.kind-plugin");
    }

    #[test]
    fn kind_tokens_round_trip() {
        for kind in NativeNotificationKind::ALL {
            assert_eq!(NativeNotificationKind::from_token(kind.as_str()), Some(kind));
        }
        assert_eq!(NativeNotificationKind::from_token("nope"), None);
    }
}
