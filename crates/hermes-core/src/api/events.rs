//! Typed payloads for gateway notifications and server->client requests that no other area
//! module owns (subagents, MoA, notices, tips, skins, cron/platform/skills change events, …).
//!
//! Two halves, both pure — no I/O, no UI:
//!
//! * **Notifications.** Every `{"method":"event","params":{type, session_id, seq, payload}}`
//!   frame arrives as a [`crate::protocol::GatewayEvent`]. [`KnownEvent::parse`] turns the
//!   event name plus its `payload` object into one typed variant. Unknown names return
//!   `None` so a newer backend never breaks a caller.
//!   Wire source: `x-notifications` in `apps/shared/src/gateway-contract.openrpc.json` and
//!   the pydantic models in `tui_gateway/contracts/events.py`; each struct names the emitter
//!   it was typed from where the shape is non-obvious.
//! * **Server->client requests.** The backend blocks a turn on these the way it blocks on
//!   `clarify`: [`parse_server_request`] lifts the params into a typed variant and the
//!   `*_reply` builders produce the exact result value to hand to
//!   [`crate::rpc::Client::respond`]. `setup_beat`, `theme.pick` and `vault.save_login`
//!   wait for a person, so an unanswered request stalls the tool for minutes; the `*_skip`
//!   / `*_cancelled` / `*_none` builders are the ways to close one without an answer.
//!
//! Every payload struct is lenient (`#[serde(default)]`, unknown fields ignored, `Option<T>`
//! wherever the emitter can omit or null a field) so an older desktop keeps decoding a
//! newer backend's frames — and vice versa.
//!
//! **Secrets.** `vault.code` returns a one-time/2FA code and `vault.save_login` returns a
//! password. Those values travel in [`SecretValue`], whose `Debug` redacts them; the reply
//! builders are the only place they are serialized onto the wire. Never log, display or
//! store them.

use std::collections::BTreeMap;

use serde_json::{Value, json};

// ── Payload decoding ──────────────────────────────────────────────────────────
//
// Payloads come from `serde_json::Value` because they ride inside one envelope. The structs
// below own the leniency (`#[serde(default)]`, `Option<T>` for anything the emitter can
// omit), so a missing, null or extra field never fails a decode.

/// Decode one payload object into `T` (`None` when the shape is unusable).
fn de<T: serde::de::DeserializeOwned>(payload: &Value) -> Option<T> {
    serde_json::from_value(payload.clone()).ok()
}

// ── Shared payload shapes ─────────────────────────────────────────────────────

/// `cron.changed`, `pairing.changed`, `platforms.changed`, `skills.changed`: a bare
/// "refetch this list" signal. The payload is `{}` today and may grow, so nothing is typed
/// into it on purpose.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(default)]
pub struct ChangeSignal {}

/// `voice.interrupted`: barge-in; the event carries no payload.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(default)]
pub struct NoPayload {}

/// The active skin (`tui_gateway/change_watcher.py::resolve_skin`). Colour maps are
/// token → colour string; `{}` when the skin engine failed to load.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct Skin {
    pub name: String,
    pub description: String,
    pub colors: BTreeMap<String, String>,
    pub light_colors: BTreeMap<String, String>,
    pub dark_colors: BTreeMap<String, String>,
    pub branding: BTreeMap<String, String>,
    pub banner_logo: String,
    pub banner_hero: String,
    pub tool_prefix: String,
    pub help_header: String,
}

// ── gateway lifecycle ─────────────────────────────────────────────────────────

/// `gateway.ready` — the first frame of every connection.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct GatewayReady {
    pub skin: Skin,
    /// The backend emits `*.changed` signals; when false the client must poll instead.
    pub change_events: bool,
    /// Replay watermark: an event with a lower epoch is from a previous backend process.
    pub replay_epoch: String,
    /// WebSocket transport only: the server answers `gateway.ping`.
    pub heartbeat: Option<bool>,
    /// The backend speaks the canonical session verbs (`runtime.describe`, …).
    pub session_authority: Option<bool>,
}

/// `setup.ready` — the free-tier bootstrap record (`hermes_cli/free_tier_bootstrap.py`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SetupReady {
    pub provider_configured: bool,
    pub inference_provider: String,
    pub free_tier_account: bool,
    pub free_tier_route: bool,
    pub has_identity: bool,
    pub other_providers: bool,
    /// Empty on success; the human-readable failure otherwise.
    pub error: String,
    /// `anon_auth.MintFailure` code, present only when the mint did not happen.
    pub error_code: Option<String>,
    pub retryable: Option<bool>,
    pub retry_after: Option<i64>,
    pub finished_at: f64,
}

// ── subagents ─────────────────────────────────────────────────────────────────

/// One entry of a child's observable output tail.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SubagentOutputTail {
    pub tool: String,
    pub preview: String,
    pub is_error: bool,
}

/// Every `subagent.*` frame (`tool_progress.py::_progress_subagent`). Status values:
/// `queued`, `running`, `completed`, `failed`, `error`, `timeout`, `interrupted`.
/// The identity fields are optional: older emitters omit them.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SubagentEvent {
    pub goal: String,
    pub task_count: i64,
    pub task_index: i64,
    pub subagent_id: Option<String>,
    pub parent_id: Option<String>,
    pub child_session_id: Option<String>,
    pub delegation_id: Option<String>,
    pub depth: Option<i64>,
    pub model: Option<String>,
    pub tool_count: Option<i64>,
    pub toolsets: Option<Vec<String>>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub reasoning_tokens: Option<i64>,
    pub api_calls: Option<i64>,
    pub files_read: Option<Vec<String>>,
    pub files_written: Option<Vec<String>>,
    pub output_tail: Option<Vec<SubagentOutputTail>>,
    /// `subagent.tool` only.
    pub tool_name: Option<String>,
    /// `subagent.thinking` / `subagent.tool` / `subagent.progress` text.
    pub text: Option<String>,
    pub status: Option<String>,
    pub summary: Option<String>,
    pub duration_seconds: Option<f64>,
    pub tool_preview: Option<String>,
}

// ── mixture-of-agents ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct MoaAggregating {
    pub aggregator: String,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct MoaPhase {
    pub phase: String,
    pub refs_done: Option<i64>,
    pub refs_total: Option<i64>,
    pub aggregator: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct MoaProgress {
    pub label: String,
    pub refs_done: i64,
    pub refs_total: i64,
}

/// One reference model's output (lands in the reasoning disclosure).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct MoaReference {
    pub label: String,
    pub text: String,
    pub index: Option<i64>,
    pub count: Option<i64>,
}

// ── notices, tips, panes ──────────────────────────────────────────────────────

/// `notification.show` — a keyed out-of-band notice (`AgentNotice`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct NotificationShow {
    pub text: String,
    /// `info` | `warn` | `error` | `success`.
    pub level: String,
    /// `ttl` | `sticky` | `status`.
    pub kind: String,
    pub ttl_ms: Option<i64>,
    pub key: Option<String>,
    pub id: Option<String>,
    pub detail: Option<String>,
}

/// `notification.clear` — withdraw the notice with this key.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct NotificationClear {
    pub key: String,
}

/// `tip.show` — a one-line tip pointed at a desktop element.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct TipShow {
    pub selector: String,
    pub text: String,
    pub title: Option<String>,
    /// `top` | `bottom` | `left` | `right`.
    pub side: Option<String>,
}

/// `pane.reveal` — focus / reveal a named desktop pane.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct PaneReveal {
    pub pane: String,
}

/// `reaction` — an affection reaction detected in the user's message (`ily`, `<3`, …).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ReactionEvent {
    pub kind: String,
}

/// `onboarding.ended` — a first-run onboarding run ended. Outcome: `done`, `skipped`,
/// `replaced`, `deleted`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct OnboardingEnded {
    pub run_id: String,
    pub stored_session_id: String,
    pub outcome: String,
}

// ── message stream side channels ──────────────────────────────────────────────

/// `message.interim` — assistant commentary beside tool calls, sealed as its own segment.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct MessageInterim {
    pub text: String,
    /// True when the same text already arrived as `message.delta` — render it once.
    pub already_streamed: bool,
}

/// One reaction on a transcript row (`tools/react_to_message_tool.py`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct MessageReaction {
    pub emoji: String,
    pub author: String,
    pub at: Option<f64>,
    pub seen: Option<bool>,
}

/// `message.reaction` — the agent reacted to a message; paint it live.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct MessageReactionEvent {
    pub row_id: i64,
    pub reactions: Vec<MessageReaction>,
    pub role: String,
}

/// `reasoning.available` — a completed reasoning block from a non-streaming provider.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ReasoningAvailable {
    pub text: String,
    pub rendered: Option<String>,
    pub verbose: Option<bool>,
}

/// `review.summary` — the background review of the last turn finished.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ReviewSummary {
    pub text: String,
}

// ── terminal / preview side agents ────────────────────────────────────────────

/// `agent.terminal.output` — output chunk from an agent-owned background process.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct TerminalOutput {
    pub process_id: String,
    pub chunk: String,
}

/// `terminal.close` — an agent-owned background process closed.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct TerminalClose {
    pub process_id: String,
}

/// `agent.print` — a status line the agent printed during a turn.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct AgentPrint {
    pub text: String,
}

/// `preview.restart.progress` — a progress line from the hidden preview-restart agent.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct PreviewRestartProgress {
    pub task_id: String,
    pub text: String,
    pub level: Option<String>,
}

/// `preview.restart.complete` — the preview-restart agent finished (`SideAgentCompletePayload`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct PreviewRestartComplete {
    pub task_id: String,
    pub text: String,
    pub question: Option<String>,
}

/// `browser.progress` — browser (CDP) connect / install progress.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct BrowserProgress {
    pub message: String,
    /// `info` | `warn` | `error`.
    pub level: String,
}

// ── approvals ─────────────────────────────────────────────────────────────────

/// `approval.cancelled` — pending approvals were dropped by interrupt/reap/teardown, so
/// their wait resolved as deny. Not a user refusal: say so in the transcript.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ApprovalCancelled {
    pub session_id: String,
    pub stored_session_id: String,
    /// `interrupt` | `reap` | `teardown` | …
    pub reason: String,
    pub cancelled_count: i64,
    pub request_ids: Vec<String>,
}

// ── connectors ────────────────────────────────────────────────────────────────

/// One leg of a connection operation (`Target.snapshot`). The minted link rides here,
/// never in the model result.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ConnectionTarget {
    pub name: String,
    /// `connector` | `mcp` | `plugin` | `skill`.
    pub kind: String,
    /// `authorize` | `connect` | `enable` | `install` | `reconnect`.
    pub action: String,
    /// `pending` | `initiated` | `connected` | `skipped` | `failed` | `expired` |
    /// `not_connected`.
    pub state: String,
    pub resolved: Option<bool>,
    pub detail: Option<String>,
    pub instructions: Option<String>,
    pub discovery_error: Option<String>,
    pub connect_url: Option<String>,
    pub connection_id: Option<String>,
    pub alias: Option<String>,
    pub attempt: Option<i64>,
    pub required_env: Option<Vec<String>>,
    pub tools: Option<Vec<String>>,
    pub hint: Option<String>,
    pub description: Option<String>,
    pub tier: Option<String>,
    pub platforms: Option<Vec<String>>,
    pub repo: Option<String>,
    pub phase: Option<String>,
    pub approved: Option<bool>,
    pub enabled: Option<bool>,
    pub already_installed: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ConnectionDisplay {
    pub suggested: Vec<String>,
}

/// Who owns a connection operation: `{type:"session", session_id}` or `{type:"account"}`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ConnectionOwner {
    #[serde(rename = "type")]
    pub kind: String,
    pub session_id: Option<String>,
}

/// `connection.request` — a connection operation opened; the desktop renders its card.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ConnectionRequest {
    pub op_id: String,
    pub seq: i64,
    pub deadline_at: f64,
    pub timeout_seconds: f64,
    pub targets: Vec<ConnectionTarget>,
    pub tool_call_id: Option<String>,
    pub display: Option<ConnectionDisplay>,
}

/// `connection.update` — one target transition (`target`/`from`/`to`/`actor`) or the
/// settlement (none of those), always with the full snapshot.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ConnectionUpdate {
    pub op_id: String,
    pub seq: i64,
    pub deadline_at: f64,
    pub settled: bool,
    pub settled_at: Option<f64>,
    /// `all_resolved` | `continue` | `deadline` | `interrupt`.
    pub settled_by: Option<String>,
    pub targets: Vec<ConnectionTarget>,
    pub owner: Option<ConnectionOwner>,
    pub target: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub actor: Option<String>,
    pub detail: Option<String>,
}

// ── voice ─────────────────────────────────────────────────────────────────────

/// `voice.status` — recorder state (`idle` | `listening` | `recording` | `transcribing` |
/// `stopped`; treat unknown states as idle).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct VoiceStatus {
    pub state: String,
}

/// `voice.partial` — live STT text while the user is still speaking.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct VoicePartial {
    pub text: String,
}

/// `voice.transcript` — a capture produced text (or a stop phrase / the silence limit).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct VoiceTranscript {
    pub text: Option<String>,
    pub stop_phrase: Option<bool>,
    pub typed: Option<bool>,
    pub no_speech_limit: Option<bool>,
}

/// `wake.detected` — a wake phrase fired.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct WakeDetected {
    pub phrase: String,
    pub profile: Option<String>,
    pub start_new_session: bool,
}

// ── todos ─────────────────────────────────────────────────────────────────────

/// One todo item (`TodoStore._validate`): `{id, content, status, parent?}`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct TodoItem {
    pub id: String,
    pub content: String,
    /// `pending` | `in_progress` | `completed` | `cancelled` (unknown → `pending`).
    pub status: String,
    pub parent: Option<String>,
}

/// `todo.updated` — a full snapshot, `revision`-monotonic, so clients reconcile atomically.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct TodoUpdated {
    pub todos: Vec<TodoItem>,
    pub revision: i64,
}

/// `tool.output_risk` — tool output classified as risky (prompt injection / secrets).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ToolOutputRisk {
    pub tool_id: String,
    pub name: String,
    /// `low` | `medium` | `high`.
    pub risk: String,
    pub findings: Vec<String>,
    pub redacted: bool,
}

// ── persisted session control ─────────────────────────────────────────────────

/// The frontend-safe goal state (`methods_session_control.py::_safe_goal_snapshot`).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct GoalSnapshot {
    pub title: String,
    pub status: String,
    pub turns_used: i64,
    pub max_turns: i64,
    /// Gate shape is owned by the goal engine; kept opaque on purpose.
    pub gates: Vec<Value>,
    pub subgoals: Vec<String>,
    pub created_at: Option<f64>,
    pub updated_at: Option<f64>,
    pub paused_reason: Option<String>,
    pub last_verdict: Option<String>,
    pub last_reason: Option<String>,
}

/// The persisted loop state; the loop's route is deliberately not part of it.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct LoopSnapshot {
    pub prompt: String,
    pub status: String,
    pub mode: String,
    pub interval_seconds: f64,
    pub current_delay: f64,
    pub times: i64,
    pub until: String,
    pub max_ticks: i64,
    pub ticks_fired: i64,
    pub created_at: f64,
    pub last_fired_at: f64,
    pub next_due_at: f64,
    pub awaiting_response: bool,
    pub deferred_by_goal: bool,
    pub paused_reason: Option<String>,
    pub last_stop_reason: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct HeartbeatSnapshot {
    pub prompt: String,
    pub status: String,
    pub interval_seconds: i64,
    pub created_at: f64,
    pub last_fired_at: f64,
    pub fire_count: i64,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SessionControl {
    pub goal: Option<GoalSnapshot>,
    pub loop_state: Option<LoopSnapshot>,
    pub heartbeat: Option<HeartbeatSnapshot>,
    pub revision: String,
    pub updated_at: f64,
}

/// `session.control.update` — persisted goal / loop / heartbeat state changed.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SessionControlUpdate {
    pub control: SessionControl,
}

// ── session lifecycle ─────────────────────────────────────────────────────────

/// `session.reclaimed` — the backend reclaimed a live session out from under its clients.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SessionReclaimed {
    pub session_id: String,
    pub stored_session_id: String,
    /// `reaped` | `evicted` | `restarted` | …
    pub reason: String,
}

/// `session.resume_progress` — deferred resume hydration progress. Status: `loading`,
/// `complete`, `failed`.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SessionResumeProgress {
    /// `history` | `tools` | …
    pub phase: String,
    pub status: String,
    pub message_count: Option<i64>,
    pub message: Option<String>,
}

// ── the event registry ────────────────────────────────────────────────────────

/// Declares, in one place, the event name → payload type mapping, the `pub const` for each
/// name, the [`KnownEvent`] enum and the `parse`/`name` pair, so a const can never drift
/// from the arm that matches it.
macro_rules! known_events {
    ($( $(#[$meta:meta])* $variant:ident, $const_name:ident, $name:literal, $ty:ty; )*) => {
        $(
            $(#[$meta])*
            pub const $const_name: &str = $name;
        )*

        /// One typed gateway notification.
        #[derive(Debug, Clone, PartialEq)]
        pub enum KnownEvent {
            $( $variant($ty), )*
        }

        impl KnownEvent {
            /// Typed payload for `event_type`, or `None` for a name this module does not
            /// know (forward compatible: the caller just ignores it).
            pub fn parse(event_type: &str, payload: &Value) -> Option<KnownEvent> {
                Some(match event_type {
                    $( $name => KnownEvent::$variant(de(payload)?), )*
                    _ => return None,
                })
            }

            /// The wire name of this event.
            pub fn name(&self) -> &'static str {
                match self {
                    $( KnownEvent::$variant(_) => $name, )*
                }
            }
        }
    };
}

known_events! {
    /// `agent.print` — a status line the agent printed during a turn.
    AgentPrint, AGENT_PRINT, "agent.print", AgentPrint;
    /// `agent.terminal.output` — output chunk from an agent-owned background process.
    AgentTerminalOutput, AGENT_TERMINAL_OUTPUT, "agent.terminal.output", TerminalOutput;
    /// `approval.cancelled` — pending approvals dropped; the wait resolved as deny.
    ApprovalCancelled, APPROVAL_CANCELLED, "approval.cancelled", ApprovalCancelled;
    /// `browser.progress` — browser connect / install progress.
    BrowserProgress, BROWSER_PROGRESS, "browser.progress", BrowserProgress;
    /// `connection.request` — a connection operation opened; render its card.
    ConnectionRequest, CONNECTION_REQUEST, "connection.request", ConnectionRequest;
    /// `connection.update` — a transition or the settlement of a connection operation.
    ConnectionUpdate, CONNECTION_UPDATE, "connection.update", ConnectionUpdate;
    /// `cron.changed` — `cron/jobs.json` moved; refetch the cron list.
    CronChanged, CRON_CHANGED, "cron.changed", ChangeSignal;
    /// `gateway.ready` — first frame of a connection.
    GatewayReady, GATEWAY_READY, "gateway.ready", GatewayReady;
    /// `message.interim` — interim assistant commentary sealed as its own segment.
    MessageInterim, MESSAGE_INTERIM, "message.interim", MessageInterim;
    /// `message.reaction` — the agent reacted to a message.
    MessageReaction, MESSAGE_REACTION, "message.reaction", MessageReactionEvent;
    /// `moa.aggregating` — the MoA aggregator started.
    MoaAggregating, MOA_AGGREGATING, "moa.aggregating", MoaAggregating;
    /// `moa.phase` — MoA phase transition.
    MoaPhase, MOA_PHASE, "moa.phase", MoaPhase;
    /// `moa.progress` — MoA reference fan-out progress.
    MoaProgress, MOA_PROGRESS, "moa.progress", MoaProgress;
    /// `moa.reference` — one MoA reference model's output.
    MoaReference, MOA_REFERENCE, "moa.reference", MoaReference;
    /// `notification.clear` — withdraw the notice with this key.
    NotificationClear, NOTIFICATION_CLEAR, "notification.clear", NotificationClear;
    /// `notification.show` — show / replace a keyed out-of-band notice.
    NotificationShow, NOTIFICATION_SHOW, "notification.show", NotificationShow;
    /// `onboarding.ended` — a first-run onboarding run ended.
    OnboardingEnded, ONBOARDING_ENDED, "onboarding.ended", OnboardingEnded;
    /// `pairing.changed` — pairing state moved; refetch pairing.
    PairingChanged, PAIRING_CHANGED, "pairing.changed", ChangeSignal;
    /// `pane.reveal` — focus / reveal a named desktop pane.
    PaneReveal, PANE_REVEAL, "pane.reveal", PaneReveal;
    /// `platforms.changed` — `gateway_state.json` moved; refetch platform status.
    PlatformsChanged, PLATFORMS_CHANGED, "platforms.changed", ChangeSignal;
    /// `preview.restart.complete` — the hidden preview-restart agent finished.
    PreviewRestartComplete, PREVIEW_RESTART_COMPLETE, "preview.restart.complete", PreviewRestartComplete;
    /// `preview.restart.progress` — a progress line from the preview-restart agent.
    PreviewRestartProgress, PREVIEW_RESTART_PROGRESS, "preview.restart.progress", PreviewRestartProgress;
    /// `reaction` — an affection reaction detected in the user's message.
    Reaction, REACTION, "reaction", ReactionEvent;
    /// `reasoning.available` — a completed reasoning block.
    ReasoningAvailable, REASONING_AVAILABLE, "reasoning.available", ReasoningAvailable;
    /// `review.summary` — the background review of the last turn finished.
    ReviewSummary, REVIEW_SUMMARY, "review.summary", ReviewSummary;
    /// `session.control.update` — persisted goal / loop / heartbeat state changed.
    SessionControlUpdate, SESSION_CONTROL_UPDATE, "session.control.update", SessionControlUpdate;
    /// `session.reclaimed` — the backend reclaimed a live session.
    SessionReclaimed, SESSION_RECLAIMED, "session.reclaimed", SessionReclaimed;
    /// `session.resume_progress` — deferred resume hydration progress.
    SessionResumeProgress, SESSION_RESUME_PROGRESS, "session.resume_progress", SessionResumeProgress;
    /// `setup.ready` — the free-tier bootstrap finished.
    SetupReady, SETUP_READY, "setup.ready", SetupReady;
    /// `skills.changed` — a skill root moved; refetch the command catalog.
    SkillsChanged, SKILLS_CHANGED, "skills.changed", ChangeSignal;
    /// `skin.changed` — the active skin moved; repaint from this palette.
    SkinChanged, SKIN_CHANGED, "skin.changed", Skin;
    /// `subagent.complete` — a child finished (status + observability rollup).
    SubagentComplete, SUBAGENT_COMPLETE, "subagent.complete", SubagentEvent;
    /// `subagent.progress` — batched tool-name progress from a child.
    SubagentProgress, SUBAGENT_PROGRESS, "subagent.progress", SubagentEvent;
    /// `subagent.spawn_requested` — `delegate_task` accepted a child goal.
    SubagentSpawnRequested, SUBAGENT_SPAWN_REQUESTED, "subagent.spawn_requested", SubagentEvent;
    /// `subagent.start` — a delegated child started running.
    SubagentStart, SUBAGENT_START, "subagent.start", SubagentEvent;
    /// `subagent.thinking` — a child's reasoning chunk.
    SubagentThinking, SUBAGENT_THINKING, "subagent.thinking", SubagentEvent;
    /// `subagent.tool` — a child called a tool.
    SubagentTool, SUBAGENT_TOOL, "subagent.tool", SubagentEvent;
    /// `terminal.close` — an agent-owned background process closed.
    TerminalClose, TERMINAL_CLOSE, "terminal.close", TerminalClose;
    /// `tip.show` — point at a desktop element with a one-line tip bubble.
    TipShow, TIP_SHOW, "tip.show", TipShow;
    /// `todo.updated` — a full todo snapshot after a todo tool ran.
    TodoUpdated, TODO_UPDATED, "todo.updated", TodoUpdated;
    /// `tool.output_risk` — tool output was classified as risky.
    ToolOutputRisk, TOOL_OUTPUT_RISK, "tool.output_risk", ToolOutputRisk;
    /// `voice.interrupted` — barge-in; the interjection interrupted the turn.
    VoiceInterrupted, VOICE_INTERRUPTED, "voice.interrupted", NoPayload;
    /// `voice.partial` — live STT text while the user is still speaking.
    VoicePartial, VOICE_PARTIAL, "voice.partial", VoicePartial;
    /// `voice.status` — voice recorder state changed.
    VoiceStatus, VOICE_STATUS, "voice.status", VoiceStatus;
    /// `voice.transcript` — a voice capture produced text (or a stop phrase).
    VoiceTranscript, VOICE_TRANSCRIPT, "voice.transcript", VoiceTranscript;
    /// `wake.detected` — a wake phrase fired.
    WakeDetected, WAKE_DETECTED, "wake.detected", WakeDetected;
}

// ── server->client requests ───────────────────────────────────────────────────

/// The onboarding beats the backend can show (`agent/onboarding_flow.py::BEATS`).
pub const SETUP_BEATS: [&str; 7] = ["name", "theme", "layout", "connectors", "plugins", "tour", "build"];

pub fn is_setup_beat(beat: &str) -> bool {
    SETUP_BEATS.contains(&beat)
}

/// `setup_beat` params: one onboarding card, blocking until answered (300 s).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct SetupBeatParams {
    pub session_id: String,
    /// One of [`SETUP_BEATS`]; unknown beats are still parsed (newer backends add some).
    pub beat: String,
    /// The beat's question, when it has one.
    pub question: Option<String>,
    pub tool_call_id: Option<String>,
    /// The card's options, shaped per beat: `{suggested: "…"}` (name), `{picker: "accent"}`
    /// (theme), `{suggested: [...]}` (layout/connectors), `{plugins: [...]}`. Kept opaque
    /// because the union has no single field set.
    pub card: Value,
}

/// `terminal.read` params: the visible in-app terminal buffer (line window optional).
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct TerminalReadParams {
    pub session_id: String,
    /// First line, 0-based from the oldest scrollback line.
    pub start: Option<i64>,
    /// Lines to read (defaults to the visible row count).
    pub count: Option<i64>,
}

/// `theme.apply` params: set_theme values for the window showing the session.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ThemeApplyParams {
    pub session_id: String,
    pub accent: Option<String>,
    /// `light` | `dark` | `system`.
    pub mode: Option<String>,
    pub layout: Option<String>,
}

/// `theme.pick` params: an accent or layout picker card that waits for the user.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct ThemePickParams {
    pub session_id: String,
    /// `accent` | `layout`.
    pub picker: String,
}

/// `vault.code` params: a one-time / 2FA code the user reads from their device.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct VaultCodeParams {
    pub session_id: String,
    pub site: Option<String>,
    pub hint: Option<String>,
}

/// `vault.save_login` params: offer to save a login the agent is about to fill.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct VaultSaveLoginParams {
    pub session_id: String,
    pub origin: String,
    pub site: String,
}

/// `window.read` params: enumerate the native window below the app. No options.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default)]
pub struct WindowReadParams {
    pub session_id: String,
}

/// One server->client request this module can answer.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerRequest {
    SetupBeat(SetupBeatParams),
    TerminalRead(TerminalReadParams),
    ThemeApply(ThemeApplyParams),
    ThemePick(ThemePickParams),
    VaultCode(VaultCodeParams),
    VaultSaveLogin(VaultSaveLoginParams),
    WindowRead(WindowReadParams),
}

/// Methods [`parse_server_request`] understands.
pub fn handles_server_request(method: &str) -> bool {
    matches!(
        method,
        "setup_beat"
            | "terminal.read"
            | "theme.apply"
            | "theme.pick"
            | "vault.code"
            | "vault.save_login"
            | "window.read"
    )
}

/// Parse one server->client request. `None` for a method another module owns (`clarify`,
/// `approval`, `preview.*`, `tour`, …) so the caller can route it on.
pub fn parse_server_request(method: &str, params: &Value) -> Option<ServerRequest> {
    Some(match method {
        "setup_beat" => ServerRequest::SetupBeat(de(params)?),
        "terminal.read" => ServerRequest::TerminalRead(de(params)?),
        "theme.apply" => ServerRequest::ThemeApply(de(params)?),
        "theme.pick" => ServerRequest::ThemePick(de(params)?),
        "vault.code" => ServerRequest::VaultCode(de(params)?),
        "vault.save_login" => ServerRequest::VaultSaveLogin(de(params)?),
        "window.read" => ServerRequest::WindowRead(de(params)?),
        _ => return None,
    })
}

// ── secrets ───────────────────────────────────────────────────────────────────

/// A value that must never be printed: one-time codes, vault passwords.
///
/// `Debug` redacts it so `{:?}` on a request, params struct or error can never leak it.
/// Read it out with [`SecretValue::expose`] only to build the wire reply.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SecretValue(String);

impl SecretValue {
    pub fn new(value: impl Into<String>) -> Self {
        SecretValue(value.into())
    }

    /// The plaintext. Never log the result.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.trim().is_empty()
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl std::fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretValue(<redacted>)")
    }
}

impl From<&str> for SecretValue {
    fn from(value: &str) -> Self {
        SecretValue::new(value)
    }
}

impl From<String> for SecretValue {
    fn from(value: String) -> Self {
        SecretValue(value)
    }
}

// ── reply builders ────────────────────────────────────────────────────────────
//
// Wire rule (`tui_gateway/server.py::_ask`): the prompts whose answer is one string reply
// `{"value": text}`; for a GUI read the text is itself JSON, so the tool can hand it to the
// model. `theme.pick` and `setup_beat` reply structured objects instead.

/// `{"value": "<json text>"}` for the reads whose answer is a JSON document.
fn value_json<T: serde::Serialize>(v: &T) -> Value {
    json!({"value": serde_json::to_string(v).unwrap_or_default()})
}

/// `setup_beat`: reply with the card's answer (`setup_name_answer` / `setup_theme_answer` /
/// `setup_layout_answer` / `setup_connectors_answer` / … build it).
pub fn setup_beat_answer(answer: Value) -> Value {
    json!({"answer": answer})
}

/// `setup_beat` name card answer.
pub fn setup_name_answer(name: &str) -> Value {
    json!({"name": name})
}

/// `setup_beat` theme card answer; `None` fields are omitted (the backend keeps its value).
pub fn setup_theme_answer(accent: Option<&str>, mode: Option<&str>) -> Value {
    let mut out = serde_json::Map::new();
    if let Some(a) = accent {
        out.insert("accent".into(), json!(a));
    }
    if let Some(m) = mode {
        out.insert("mode".into(), json!(m));
    }
    Value::Object(out)
}

/// `setup_beat` layout card answer.
pub fn setup_layout_answer(layout: &str) -> Value {
    json!({"layout": layout})
}

/// `setup_beat` connectors card answer (ids from the card's options).
pub fn setup_connectors_answer(connectors: &[String]) -> Value {
    json!({"connectors": connectors})
}

/// `setup_beat` plugins card answer (ids from the card's options).
pub fn setup_plugins_answer(plugins: &[String]) -> Value {
    json!({"plugins": plugins})
}

/// `setup_beat`: the user typed this text (the name) while the card waited.
pub fn setup_beat_said(said: &str) -> Value {
    json!({"said": said})
}

/// `setup_beat`: the user sent a message instead of answering; the backend ends the turn.
pub fn setup_beat_wrote_instead() -> Value {
    json!({"wrote_instead": true})
}

/// `setup_beat`: close the card without an answer.
pub fn setup_beat_skip() -> Value {
    json!({})
}

/// One terminal pane read (`tools/read_terminal_tool.py` result shape).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct TerminalRead {
    pub total_lines: i64,
    /// First line of the returned window, 0-based from the oldest line.
    pub start: i64,
    pub end: i64,
    pub viewport_rows: i64,
    pub cursor_row: i64,
    /// The lines, joined by `\n`.
    pub text: String,
}

/// `terminal.read` answer: the read, serialized into `{"value": …}`.
pub fn terminal_read_reply(read: &TerminalRead) -> Value {
    value_json(read)
}

/// `terminal.read` answer when no terminal pane is open (the tool reports "nothing open").
pub fn terminal_read_none() -> Value {
    json!({"value": ""})
}

/// Theme values a theme card / apply reports back. Unset values are omitted on the wire
/// (the Electron `applied` object only carries the keys it changed).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ThemeValues {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,
    /// `light` | `dark` | `system`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
}

/// `theme.apply` answer on success: `{"value": {"success": true, "applied": {...}}}`.
pub fn theme_apply_reply(applied: &ThemeValues) -> Value {
    value_json(&json!({"success": true, "applied": applied}))
}

/// `theme.apply` answer on refusal, e.g. the user is looking at another session. `available`
/// optionally lists the accepted values.
pub fn theme_apply_error(message: &str, available: &[String]) -> Value {
    let mut body = json!({"success": false, "error": message});
    if !available.is_empty() {
        body["available"] = json!(available);
    }
    value_json(&body)
}

/// `theme.pick`: the user picked these values.
pub fn theme_pick_picked(values: &ThemeValues) -> Value {
    json!({"picked": values})
}

/// `theme.pick`: the user typed this instead of picking.
pub fn theme_pick_said(said: &str) -> Value {
    json!({"said": said})
}

/// `theme.pick`: dismissed (or unusable params) — the card closes, the tool reports cancel.
pub fn theme_pick_cancelled() -> Value {
    json!({"cancelled": true})
}

/// `vault.code` answer: the one-time code. An empty code means the user skipped.
pub fn vault_code_reply(code: &SecretValue) -> Value {
    json!({"value": code.expose()})
}

/// `vault.save_login` answer: identifier + password as **JSON text** under `value`; the
/// backend hands it straight to the vault store, so `{}`/`""` would be a discarded login.
pub fn vault_save_login_reply(identifier: &str, password: &SecretValue) -> Value {
    let body = json!({"identifier": identifier, "password": password.expose()});
    json!({"value": body.to_string()})
}

/// `vault.save_login` answer that declines the save.
pub fn vault_save_login_declined() -> Value {
    json!({"value": ""})
}

/// A native window rectangle.
#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct WindowBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// The window directly underneath the app (`apps/desktop/electron/window-below.ts`).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct WindowBelow {
    pub window: Option<BelowWindow>,
    pub frontmost: Option<FrontWindow>,
    pub platform: String,
    /// Set when the OS withholds something (e.g. a macOS title without screen recording).
    pub note: Option<String>,
    /// Set when windows cannot be enumerated at all; says what would fix it.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct BelowWindow {
    pub app: String,
    pub title: String,
    pub id: Option<i64>,
    pub bounds: WindowBounds,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct FrontWindow {
    pub app: String,
    pub title: String,
}

/// `window.read` answer: the enumeration, serialized into `{"value": …}`.
pub fn window_read_reply(below: &WindowBelow) -> Value {
    value_json(below)
}

/// `window.read` answer when enumeration is unavailable (the tool reports it).
pub fn window_read_none() -> Value {
    json!({"value": ""})
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// One realistic payload per notification, taken from the emitter / contract example.
    /// Keys are the wire names; `KnownEvent::parse` must accept every one.
    fn samples() -> Vec<(&'static str, Value)> {
        vec![
            ("agent.print", json!({"text": "Checked 3 files, no drift."})),
            (
                "agent.terminal.output",
                json!({"process_id": "proc-7", "chunk": "building…\n"}),
            ),
            (
                "approval.cancelled",
                json!({"session_id": "s1", "stored_session_id": "st-1", "reason": "interrupt",
                       "cancelled_count": 2, "request_ids": ["srq-a", "srq-b"]}),
            ),
            (
                "browser.progress",
                json!({"message": "Installing Chrome…", "level": "info"}),
            ),
            (
                "connection.request",
                json!({"op_id": "op-1", "seq": 4, "deadline_at": 1760000000.5, "timeout_seconds": 300.0,
                       "targets": [{"name": "github", "kind": "connector", "action": "authorize",
                                    "state": "pending", "connect_url": "https://example.test/oauth",
                                    "attempt": 1}],
                       "tool_call_id": "call-1",
                       "display": {"suggested": ["github", "linear"]}}),
            ),
            (
                "connection.update",
                json!({"op_id": "op-1", "seq": 5, "deadline_at": 1760000000.5, "settled": false,
                       "targets": [{"name": "github", "kind": "connector", "action": "authorize",
                                    "state": "connected", "resolved": true}],
                       "owner": {"type": "session", "session_id": "s1"},
                       "target": "github", "from": "initiated", "to": "connected", "actor": "user"}),
            ),
            ("cron.changed", json!({})),
            (
                "gateway.ready",
                json!({"skin": {"name": "oracle", "description": "warm serif", "colors": {"bg": "#101010"}},
                       "change_events": true, "replay_epoch": "7f3a", "heartbeat": true,
                       "session_authority": false}),
            ),
            (
                "message.interim",
                json!({"text": "Let me look at the tests.", "already_streamed": false}),
            ),
            (
                "message.reaction",
                json!({"row_id": 42, "role": "assistant",
                       "reactions": [{"emoji": "❤️", "author": "agent", "at": 1760000000.0, "seen": false}]}),
            ),
            ("moa.aggregating", json!({"aggregator": "claude-sonnet-4"})),
            (
                "moa.phase",
                json!({"phase": "aggregator", "refs_done": 3, "refs_total": 3, "aggregator": "gpt-5"}),
            ),
            (
                "moa.progress",
                json!({"label": "reference 2", "refs_done": 2, "refs_total": 4}),
            ),
            (
                "moa.reference",
                json!({"label": "opus", "text": "Two options: …", "index": 1, "count": 3}),
            ),
            ("notification.clear", json!({"key": "model_switch.dropped"})),
            (
                "notification.show",
                json!({"text": "Could not switch model: quota", "level": "warn", "kind": "ttl",
                       "ttl_ms": 12000, "key": "model_switch.dropped", "id": "model_switch.dropped"}),
            ),
            (
                "onboarding.ended",
                json!({"run_id": "run-2", "stored_session_id": "st-9", "outcome": "done"}),
            ),
            ("pairing.changed", json!({})),
            ("pane.reveal", json!({"pane": "terminal"})),
            ("platforms.changed", json!({})),
            (
                "preview.restart.complete",
                json!({"task_id": "task-1", "text": "Restarted on :3000.", "question": "Anything else?"}),
            ),
            (
                "preview.restart.progress",
                json!({"task_id": "task-1", "text": "npm run dev", "level": "info"}),
            ),
            ("reaction", json!({"kind": "hearts"})),
            (
                "reasoning.available",
                json!({"text": "The failing test is…", "rendered": "The failing test is…", "verbose": true}),
            ),
            ("review.summary", json!({"text": "💾 Saved 2 memories."})),
            (
                "session.control.update",
                json!({"control": {"revision": "9", "updated_at": 1760000000.0,
                       "goal": {"title": "Ship it", "status": "active", "turns_used": 2, "max_turns": 20,
                                "subgoals": ["tests"], "gates": [{"kind": "tests"}]},
                       "loop": {"prompt": "check ci", "status": "running", "mode": "interval",
                                "interval_seconds": 300.0, "current_delay": 300.0, "times": 0,
                                "until": "", "max_ticks": 10, "ticks_fired": 1,
                                "created_at": 1760000000.0, "last_fired_at": 1760000000.0,
                                "next_due_at": 1760000300.0, "awaiting_response": false,
                                "deferred_by_goal": false},
                       "heartbeat": {"prompt": "status?", "status": "running", "interval_seconds": 60,
                                     "created_at": 1760000000.0, "last_fired_at": 1760000000.0,
                                     "fire_count": 3}}}),
            ),
            (
                "session.reclaimed",
                json!({"session_id": "s1", "stored_session_id": "st-1", "reason": "reaped"}),
            ),
            (
                "session.resume_progress",
                json!({"phase": "history", "status": "loading", "message_count": 120}),
            ),
            (
                "setup.ready",
                json!({"provider_configured": true, "inference_provider": "nous", "free_tier_account": true,
                       "free_tier_route": true, "has_identity": true, "other_providers": false,
                       "error": "", "finished_at": 1760000000.0}),
            ),
            ("skills.changed", json!({})),
            (
                "skin.changed",
                json!({"name": "oracle", "description": "warm serif", "colors": {"bg": "#101010"},
                       "banner_logo": "HERMES"}),
            ),
            (
                "subagent.complete",
                json!({"goal": "Audit the API", "task_count": 3, "task_index": 1, "subagent_id": "sa-1",
                       "parent_id": "sa-0", "child_session_id": "s-child", "depth": 1,
                       "model": "deepseek-v4", "tool_count": 12, "toolsets": ["files"],
                       "input_tokens": 900, "output_tokens": 120, "reasoning_tokens": 40,
                       "api_calls": 5, "files_read": ["src/lib.rs"], "files_written": ["docs/x.md"],
                       "output_tail": [{"tool": "read_file", "preview": "1|…", "is_error": false}],
                       "status": "completed", "summary": "3 findings", "duration_seconds": 42.5}),
            ),
            (
                "subagent.progress",
                json!({"goal": "Audit the API", "task_count": 3, "task_index": 1,
                       "tool_name": "grep", "tool_preview": "grep -rn", "status": "running"}),
            ),
            (
                "subagent.spawn_requested",
                json!({"goal": "Audit the API", "task_count": 3, "task_index": 2}),
            ),
            (
                "subagent.start",
                json!({"goal": "Audit the API", "task_count": 3, "task_index": 0, "subagent_id": "sa-1"}),
            ),
            (
                "subagent.thinking",
                json!({"goal": "Audit the API", "task_count": 3, "task_index": 0, "text": "First, the RPC layer…"}),
            ),
            (
                "subagent.tool",
                json!({"goal": "Audit the API", "task_count": 3, "task_index": 0,
                       "tool_name": "read_file", "text": "src/rpc.rs"}),
            ),
            ("terminal.close", json!({"process_id": "proc-7"})),
            (
                "tip.show",
                json!({"selector": "#composer", "text": "Type here to talk", "title": "Composer",
                       "side": "top"}),
            ),
            (
                "todo.updated",
                json!({"revision": 4, "todos": [
                    {"id": "1", "content": "Read rpc.rs", "status": "completed"},
                    {"id": "2", "content": "Write events.rs", "status": "in_progress", "parent": "1"}]}),
            ),
            (
                "tool.output_risk",
                json!({"tool_id": "call-1", "name": "fetch", "risk": "high",
                       "findings": ["prompt injection: ignore previous instructions"], "redacted": true}),
            ),
            ("voice.interrupted", json!({})),
            ("voice.partial", json!({"text": "open the las"})),
            ("voice.status", json!({"state": "recording"})),
            (
                "voice.transcript",
                json!({"text": "open the last session", "typed": false, "stop_phrase": false}),
            ),
            (
                "wake.detected",
                json!({"phrase": "hey hermes", "profile": "default", "start_new_session": true}),
            ),
        ]
    }

    #[test]
    fn every_owned_event_parses_from_a_realistic_payload() {
        for (name, payload) in samples() {
            let ev = KnownEvent::parse(name, &payload).unwrap_or_else(|| panic!("{name} did not parse: {payload}"));
            assert_eq!(ev.name(), name, "name() must round-trip");
        }
        // The list above covers the whole owned set: no const is missing a sample.
        assert_eq!(samples().len(), 46);
    }

    #[test]
    fn unknown_and_foreign_events_are_ignored() {
        assert!(KnownEvent::parse("billing.updated", &json!({"x": 1})).is_none());
        assert!(KnownEvent::parse("pet.changed", &json!({})).is_none());
        assert!(KnownEvent::parse("", &json!({})).is_none());
    }

    #[test]
    fn payload_fields_are_typed() {
        let ev = KnownEvent::parse(
            "gateway.ready",
            &json!({"skin": {"name": "oracle", "colors": {"bg": "#101010"}},
                    "change_events": true, "replay_epoch": "7f3a", "heartbeat": true}),
        )
        .unwrap();
        match ev {
            KnownEvent::GatewayReady(p) => {
                assert_eq!(p.skin.name, "oracle");
                assert_eq!(p.skin.colors["bg"], "#101010");
                assert!(p.change_events && p.heartbeat == Some(true));
                assert_eq!(p.session_authority, None);
            }
            other => panic!("{other:?}"),
        }

        // The change signals carry no fields but must still parse from `{}`.
        assert_eq!(
            KnownEvent::parse("cron.changed", &json!({})),
            Some(KnownEvent::CronChanged(ChangeSignal {}))
        );

        // Owner is a tagged union: unknown members degrade to an empty kind.
        let ev = KnownEvent::parse(
            "connection.update",
            &json!({"op_id": "op-1", "seq": 1, "deadline_at": 1.0, "settled": true,
                    "settled_by": "deadline", "targets": [], "owner": {"type": "account"}}),
        )
        .unwrap();
        match ev {
            KnownEvent::ConnectionUpdate(p) => {
                assert!(p.settled);
                assert_eq!(p.settled_by.as_deref(), Some("deadline"));
                assert_eq!(p.owner.unwrap().kind, "account");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn subagent_rollup_and_todo_snapshot_decode() {
        let ev = KnownEvent::parse(
            "subagent.complete",
            &json!({"goal": "g", "task_count": 2, "task_index": 1, "status": "failed",
                    "output_tail": [{"tool": "shell", "preview": "exit 1", "is_error": true}],
                    "duration_seconds": 3.5, "files_written": ["a.md"]}),
        )
        .unwrap();
        match ev {
            KnownEvent::SubagentComplete(p) => {
                assert_eq!(p.task_index, 1);
                assert_eq!(p.status.as_deref(), Some("failed"));
                assert!(p.output_tail.unwrap()[0].is_error);
                assert_eq!(p.files_written.unwrap(), vec!["a.md"]);
                assert_eq!(p.tool_count, None);
            }
            other => panic!("{other:?}"),
        }

        let ev = KnownEvent::parse(
            "todo.updated",
            &json!({"revision": 2, "todos": [{"id": "1", "content": "x", "status": "pending"}]}),
        )
        .unwrap();
        match ev {
            KnownEvent::TodoUpdated(p) => {
                assert_eq!(p.revision, 2);
                assert_eq!(p.todos[0].content, "x");
                assert_eq!(p.todos[0].parent, None);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn defaults_survive_an_empty_payload() {
        // A malformed or trimmed payload must decode to defaults, not fail.
        let ev = KnownEvent::parse("notification.show", &json!({})).unwrap();
        assert_eq!(ev, KnownEvent::NotificationShow(NotificationShow::default()));
        assert!(KnownEvent::parse("voice.status", &Value::Null).is_none());
    }

    // ── server->client requests ───────────────────────────────────────────────

    #[test]
    fn server_requests_parse_into_typed_params() {
        let r = parse_server_request(
            "setup_beat",
            &json!({"session_id": "s1", "beat": "theme", "question": "Pick a theme",
                    "tool_call_id": "call-1", "card": {"picker": "accent"}}),
        )
        .unwrap();
        match r {
            ServerRequest::SetupBeat(p) => {
                assert_eq!(p.beat, "theme");
                assert!(is_setup_beat(&p.beat));
                assert_eq!(p.card["picker"], "accent");
                assert_eq!(p.session_id, "s1");
            }
            other => panic!("{other:?}"),
        }

        let r = parse_server_request("terminal.read", &json!({"session_id": "s1", "start": 10, "count": 20})).unwrap();
        match r {
            ServerRequest::TerminalRead(p) => {
                assert_eq!((p.start, p.count), (Some(10), Some(20)));
            }
            other => panic!("{other:?}"),
        }

        let r = parse_server_request(
            "theme.apply",
            &json!({"session_id": "s1", "accent": "amber", "mode": "dark", "layout": "studio"}),
        )
        .unwrap();
        match r {
            ServerRequest::ThemeApply(p) => {
                assert_eq!(p.accent.as_deref(), Some("amber"));
                assert_eq!(p.mode.as_deref(), Some("dark"));
                assert_eq!(p.layout.as_deref(), Some("studio"));
            }
            other => panic!("{other:?}"),
        }

        let r = parse_server_request("theme.pick", &json!({"session_id": "s1", "picker": "layout"})).unwrap();
        assert!(matches!(r, ServerRequest::ThemePick(p) if p.picker == "layout"));

        let r = parse_server_request(
            "vault.code",
            &json!({"session_id": "s1", "site": "bank.test", "hint": "SMS code"}),
        )
        .unwrap();
        match r {
            ServerRequest::VaultCode(p) => {
                assert_eq!(p.site.as_deref(), Some("bank.test"));
                assert_eq!(p.hint.as_deref(), Some("SMS code"));
            }
            other => panic!("{other:?}"),
        }

        let r = parse_server_request(
            "vault.save_login",
            &json!({"session_id": "s1", "origin": "https://bank.test", "site": "Bank"}),
        )
        .unwrap();
        match r {
            ServerRequest::VaultSaveLogin(p) => {
                assert_eq!(p.origin, "https://bank.test");
                assert_eq!(p.site, "Bank");
            }
            other => panic!("{other:?}"),
        }

        let r = parse_server_request("window.read", &json!({"session_id": "s1"})).unwrap();
        assert!(matches!(r, ServerRequest::WindowRead(p) if p.session_id == "s1"));
    }

    #[test]
    fn foreign_server_requests_are_left_to_other_modules() {
        assert!(parse_server_request("clarify", &json!({"session_id": "s"})).is_none());
        assert!(parse_server_request("approval", &json!({})).is_none());
        assert!(parse_server_request("preview.read", &json!({})).is_none());
        assert!(!handles_server_request("tour"));
        assert!(handles_server_request("vault.code"));
    }

    #[test]
    fn setup_beat_replies_match_the_result_schema() {
        assert_eq!(
            setup_beat_answer(setup_name_answer("Murali")),
            json!({"answer": {"name": "Murali"}})
        );
        assert_eq!(
            setup_beat_answer(setup_theme_answer(None, Some("dark"))),
            json!({"answer": {"mode": "dark"}})
        );
        assert_eq!(
            setup_beat_answer(setup_theme_answer(Some("amber"), None)),
            json!({"answer": {"accent": "amber"}})
        );
        assert_eq!(
            setup_beat_answer(setup_layout_answer("studio")),
            json!({"answer": {"layout": "studio"}})
        );
        assert_eq!(
            setup_beat_answer(setup_connectors_answer(&["github".into()])),
            json!({"answer": {"connectors": ["github"]}})
        );
        assert_eq!(
            setup_beat_answer(setup_plugins_answer(&["redline".into()])),
            json!({"answer": {"plugins": ["redline"]}})
        );
        assert_eq!(setup_beat_said("Murali"), json!({"said": "Murali"}));
        assert_eq!(setup_beat_wrote_instead(), json!({"wrote_instead": true}));
        assert_eq!(setup_beat_skip(), json!({}));
    }

    #[test]
    fn terminal_and_window_reads_reply_with_json_text() {
        let read = TerminalRead {
            total_lines: 120,
            start: 100,
            end: 110,
            viewport_rows: 24,
            cursor_row: 109,
            text: "line 100\nline 101".into(),
        };
        let v = terminal_read_reply(&read);
        let text = v["value"].as_str().unwrap();
        let back: TerminalRead = serde_json::from_str(text).unwrap();
        assert_eq!(back, read);
        assert_eq!(terminal_read_none(), json!({"value": ""}));

        let below = WindowBelow {
            window: Some(BelowWindow {
                app: "Firefox".into(),
                title: "Docs".into(),
                id: Some(7),
                bounds: WindowBounds {
                    x: 0.0,
                    y: 0.0,
                    width: 800.0,
                    height: 600.0,
                },
            }),
            frontmost: None,
            platform: "linux".into(),
            note: None,
            error: None,
        };
        let v = window_read_reply(&below);
        let back: WindowBelow = serde_json::from_str(v["value"].as_str().unwrap()).unwrap();
        assert_eq!(back, below);
        assert_eq!(window_read_none(), json!({"value": ""}));
    }

    #[test]
    fn theme_replies_match_the_answer_shapes() {
        let values = ThemeValues {
            accent: Some("amber".into()),
            mode: None,
            layout: None,
        };
        let v = theme_apply_reply(&values);
        let body: Value = serde_json::from_str(v["value"].as_str().unwrap()).unwrap();
        assert_eq!(body, json!({"success": true, "applied": {"accent": "amber"}}));

        let v = theme_apply_error("not looking at this chat", &["amber".into(), "teal".into()]);
        let body: Value = serde_json::from_str(v["value"].as_str().unwrap()).unwrap();
        assert_eq!(body["success"], json!(false));
        assert_eq!(body["available"], json!(["amber", "teal"]));
        assert_eq!(body["error"], json!("not looking at this chat"));

        assert_eq!(theme_pick_picked(&values), json!({"picked": {"accent": "amber"}}));
        assert_eq!(theme_pick_said("the blue one"), json!({"said": "the blue one"}));
        assert_eq!(theme_pick_cancelled(), json!({"cancelled": true}));
    }

    #[test]
    fn vault_replies_carry_the_secret_exactly_once() {
        let code = SecretValue::new("492013");
        assert_eq!(vault_code_reply(&code), json!({"value": "492013"}));

        let password = SecretValue::new("hunter2");
        let v = vault_save_login_reply("murali", &password);
        let body: Value = serde_json::from_str(v["value"].as_str().unwrap()).unwrap();
        assert_eq!(body, json!({"identifier": "murali", "password": "hunter2"}));

        assert_eq!(vault_save_login_declined(), json!({"value": ""}));
        assert!(vault_code_reply(&SecretValue::default())["value"] == json!(""));
    }

    #[test]
    fn secrets_never_render_in_debug_output() {
        let secret = SecretValue::new("swordfish");
        let shown = format!("{secret:?}");
        assert!(!shown.contains("swordfish"), "{shown}");
        assert!(shown.contains("redacted"));
        // A params struct carrying one must not leak through `{:?}` either.
        let params = VaultCodeParams {
            session_id: "s".into(),
            site: None,
            hint: None,
        };
        assert!(!format!("{params:?}").contains("swordfish"));
        assert_eq!(secret.expose(), "swordfish");
        assert!(SecretValue::new("   ").is_empty());
        assert_eq!(SecretValue::from("x").into_inner(), "x");
    }
}
