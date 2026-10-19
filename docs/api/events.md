# Events API (`api::events`)

Typed payloads for the gateway frames that no other area module owns: the notifications the
renderer paints without calling anything (subagents, MoA, notices, skins, change signals,
voice/wake, terminal side agents, connectors) and the server->client requests that block a
turn until this client answers (`setup_beat`, the theme cards, the vault prompts, the GUI
reads).

Sources of truth: `x-notifications` / `x-server-requests` in
`apps/shared/src/gateway-contract.openrpc.json`, `tui_gateway/contracts/events.py`,
`tui_gateway/server_requests.py`, the emitters in `tui_gateway/` (`agent_callbacks.py`,
`tool_progress.py`, `methods_*.py`, `session_registry.py`, `theme_bridge.py`), and the Electron
consumers in `apps/desktop/src/app/session/hooks/use-message-stream/gateway-event/`.

All payload structs are lenient (`#[serde(default)]`, unknown fields ignored, `Option<T>` for
anything an emitter can omit) so a newer backend never fails a decode.

## Notifications

`KnownEvent::parse(event_type, payload) -> Option<KnownEvent>` — `None` for a name this
module does not own, so the caller simply ignores it. `KnownEvent::name()` returns the wire
name. Every name has a `pub const` (e.g. `events::GATEWAY_READY`) for matches and subscriptions.

| Group | Events | Typed as |
|---|---|---|
| Lifecycle | `gateway.ready`, `setup.ready` | `GatewayReady` (with `Skin`), `SetupReady` |
| Skins / refetch signals | `skin.changed`; `cron.changed`, `pairing.changed`, `platforms.changed`, `skills.changed` | `Skin`; `ChangeSignal` (empty by design — just refetch) |
| Subagents | `subagent.start`, `.spawn_requested`, `.progress`, `.thinking`, `.tool`, `.complete` | one `SubagentEvent` (identity fields optional, `output_tail` + token rollup for `.complete`) |
| MoA | `moa.aggregating`, `moa.phase`, `moa.progress`, `moa.reference` | `MoaAggregating`, `MoaPhase`, `MoaProgress`, `MoaReference` |
| Message side channels | `message.interim`, `message.reaction`, `reasoning.available`, `review.summary`, `reaction` | `MessageInterim`, `MessageReactionEvent` (`MessageReaction`), `ReasoningAvailable`, `ReviewSummary`, `ReactionEvent` |
| Notices / desktop chrome | `notification.show`, `notification.clear`, `tip.show`, `pane.reveal`, `onboarding.ended` | `NotificationShow`, `NotificationClear`, `TipShow`, `PaneReveal`, `OnboardingEnded` |
| Terminal & preview agents | `agent.print`, `agent.terminal.output`, `terminal.close`, `preview.restart.progress`, `preview.restart.complete`, `browser.progress` | `AgentPrint`, `TerminalOutput`, `TerminalClose`, `PreviewRestartProgress`, `PreviewRestartComplete`, `BrowserProgress` |
| Approvals | `approval.cancelled` | `ApprovalCancelled` (deny-by-interrupt, not a refusal) |
| Connectors | `connection.request`, `connection.update` | `ConnectionRequest` / `ConnectionUpdate` with `ConnectionTarget`, `ConnectionOwner` (`{type:"session"\|"account"}`) |
| Voice & wake | `voice.status`, `voice.partial`, `voice.transcript`, `voice.interrupted`, `wake.detected` | `VoiceStatus`, `VoicePartial`, `VoiceTranscript`, `NoPayload`, `WakeDetected` |
| Session control | `session.control.update`, `session.reclaimed`, `session.resume_progress` | `SessionControlUpdate` (`SessionControl`, `GoalSnapshot`, `LoopSnapshot`, `HeartbeatSnapshot`), `SessionReclaimed`, `SessionResumeProgress` |
| Todos / risk | `todo.updated`, `tool.output_risk` | `TodoUpdated` (`TodoItem`, revision-monotonic), `ToolOutputRisk` |

Not owned here (other areas): `billing.*`, `free_tier.*`, `pet.*`, `display.*`, `bot_relay.*`,
`browser.controller.*`, and the transcript/typing events (`message.*` deltas, `tool.*`,
`session.info`, `approval`), which the session controller already reduces.

## Server->client requests

`parse_server_request(method, params) -> Option<ServerRequest>`; `handles_server_request(method)`
is the routing predicate. Answer with `Client::respond(id, Ok(reply))` using these builders:

| Request | Params type | Reply builder(s) | Notes |
|---|---|---|---|
| `setup_beat` | `SetupBeatParams` (`beat` ∈ `SETUP_BEATS`, opaque `card`) | `setup_beat_answer(setup_name_answer` / `setup_theme_answer` / `setup_layout_answer` / `setup_connectors_answer` / `setup_plugins_answer)`, `setup_beat_said`, `setup_beat_wrote_instead`, `setup_beat_skip` | **Blocks the turn** up to 300 s. `card` is a union per beat, so it stays `Value`. |
| `terminal.read` | `TerminalReadParams` (`start`, `count`) | `terminal_read_reply(&TerminalRead)`, `terminal_read_none()` | Reply is `{"value": "<json text>"}`; `""` = no pane open. |
| `window.read` | `WindowReadParams` | `window_read_reply(&WindowBelow)`, `window_read_none()` | Native enumeration lives in the shell; `""` = unavailable. |
| `theme.apply` | `ThemeApplyParams` (`accent`, `mode`, `layout`) | `theme_apply_reply(&ThemeValues)`, `theme_apply_error(msg, available)` | Active session only; answer carries what was actually applied. |
| `theme.pick` | `ThemePickParams` (`picker` = `accent`\|`layout`) | `theme_pick_picked`, `theme_pick_said`, `theme_pick_cancelled` | **Waits for a person** (300 s). |
| `vault.code` | `VaultCodeParams` (`site`, `hint`) | `vault_code_reply(&SecretValue)` | **Secret**: a one-time/2FA code. |
| `vault.save_login` | `VaultSaveLoginParams` (`origin`, `site`) | `vault_save_login_reply(identifier, &SecretValue)`, `vault_save_login_declined()` | **Secret**: the password goes as JSON text under `value`, straight to the vault store. |

`SecretValue` is the only carrier for those two values: its `Debug` prints
`SecretValue(<redacted>)`, so logging a params struct or an error cannot leak a code or a
password. Read it out with `expose()` only inside a reply builder.

## Run live (private test backend)

`crates/hermes-core/tests/live_api_events.rs` — up to 2 checks (the `gateway.ready` one only when the backend sends it) against `HERMES_DESKTOP_PORT=<private>`:

* the first notification after connect parses as `KnownEvent::GatewayReady` (real
  `change_events` / `replay_epoch` / `skin` from the backend; skipped with a note if a build
  opens the socket without one),
* `gateway.ping` answers `{"ok":true}`.

Every other notification needs a model turn, a recorder, a child process or a config change to
emit, and every server->client request is *sent by* the backend mid-turn — so those are
fixture-tested in `api::events` (one realistic payload per event, taken from the emit site)
and builder-only live. No live call spends a model call, touches the network, reveals a secret
or mutates state.
