# Hermes Desktop — Feature & Settings Parity Checklist

Exhaustive, source-derived inventory of every user-facing feature and setting in the Hermes Desktop app (`apps/desktop`). Built by reading the React renderer (`src/`), the Electron main process (`electron/`), the REST helpers (`src/api/*.ts`), the gateway contract (`apps/shared/src/gateway-contract.openrpc.json` — 259 RPC methods, 77 notifications, 10 server->client requests), and the settings manifests.

## Legend

| Token | Meaning |
|---|---|
| `RPC <method>` | backend call over the JSON-RPC 2.0 WebSocket (`ws://127.0.0.1:<port>/api/ws?token=…`) |
| `SRV <name>` | server->client request the desktop must answer (`approval`, `clarify`, `secret`, `sudo`, `connection.request`, `setup_choose`, `preview.act`, `preview.read`, `terminal.read`, `display.install.sudo`) |
| `WS <name>` | backend notification pushed over the WebSocket |
| `REST <path>` | HTTP endpoint under `/api/...` (via `src/api/*`) |
| `config <key>` | persisted in backend `config.yaml` (round-trips through `/api/config` or `config.set`) |
| `local` | renderer/main-process state only; no backend surface |
| `(platform)` | Electron/web-only or OS-integration behavior a native/host client would reimplement or omit |

Each line is one parity item: `- [ ] <feature> — <backend surface> — <source file ref>`.

### Rewrite status markers

| Marker | Meaning |
|---|---|
| `- [ ]` | not implemented yet |
| `- [~]` | partially implemented — the note after `⟶` says what is missing or untested |
| `- [x]` | implemented **and** exercised against the real backend (screenshot or live probe) |
| `- [-]` | explicitly unsupported (reason given) |

Status is updated in the same commit as the code it describes.

### Rewrite progress

Counted from the markers below (re-count with `grep -c '^- \[x\]' docs/PARITY.md` etc.):
**43 done · 188 partial · 561 not started · 1 unsupported** (of 793).
Performance work is tracked separately in [PERF.md](PERF.md).

## Summary

| Section | Items |
|---|---:|
| Chat & composer | 50 |
| Transcript rendering | 37 |
| Tools / terminal rows | 22 |
| Approvals / clarify | 17 |
| Sessions & sidebar | 43 |
| Projects / workspaces | 25 |
| Models & providers | 30 |
| Profiles | 22 |
| Skills | 16 |
| Tools / toolsets | 20 |
| MCP | 18 |
| Plugins / connectors | 24 |
| Memory | 14 |
| Messaging gateways | 12 |
| Scheduled / cron | 12 |
| Artifacts / preview | 19 |
| Right panel (files / review / terminal) | 26 |
| Voice | 26 |
| Command palette & shortcuts | 61 |
| Settings (shell) | 4 |
| Settings — Model page | 7 |
| Settings — Chat page | 5 |
| Settings — Appearance page | 33 |
| Settings — Workspace page | 9 |
| Settings — Safety page | 8 |
| Settings — Browser page | 3 |
| Settings — Memory & Context page | 12 |
| Settings — Voice page | 18 |
| Settings — Advanced page | 20 |
| Settings — Gateway / Connections page | 13 |
| Settings — Keybinds page | 3 |
| Settings — Notifications page | 9 |
| Settings — Sessions page | 3 |
| Settings — Vault page | 5 |
| Settings — Providers & Keys page | 6 |
| Settings — Plugins page | 3 |
| Settings — Billing page | 6 |
| Settings — About page | 5 |
| Appearance / themes | 12 |
| Notifications | 11 |
| Updates | 9 |
| Window / tray / HUD / pet | 27 |
| Onboarding / auth / billing | 17 |
| Remote connections | 20 |
| Extras: webhooks, imports, subagents, learning, telemetry | 31 |
| **Total** | **793** |

## Chat & composer

- [x] Send message / submit turn — RPC prompt.submit — src/app/session/hooks/use-prompt-actions/submit.ts ⟶ live: `HERMES_DESKTOP_AUTOSEND` run, screenshots rs-dark-chat-*.png
- [~] Steer a running turn — RPC session.steer — src/app/chat/session-tile-actions.ts ⟶ backend client ready in `api::sessions` (live-verified calls listed in docs/api/sessions.md); UI not wired
- [ ] Queue a message while busy (⌘Enter) — local + RPC prompt.submit (queue drain) — src/store/composer-queue.ts
- [ ] Send next queued message (⌘⇧K) — local — src/store/composer-queue.ts, src/lib/keybinds/actions.ts
- [x] Interrupt / stop the active turn — RPC session.interrupt — src/app/chat/session-tile-actions.ts ⟶ live 2026-10-16, `tests/live_backend.rs` vs private backend: Stop clicked mid-stream → reply stops growing (30 of 300 lines, unchanged 3 s later), running=false, no spinner, button back to Send, next prompt answered. **Esc** (global shortcut, focus in composer) verified the same way: stopped at 24 of 300 lines, unchanged 3 s later, next prompt answered
- [~] Redirect a running turn — RPC session.redirect — src/app/chat/session-tile-actions.ts ⟶ backend client ready in `api::sessions` (live-verified calls listed in docs/api/sessions.md); UI not wired
- [~] Rewind / edit-and-resend a prior user turn — RPC session.history + prompt.submit — src/app/session/hooks/use-prompt-actions/rewind.ts ⟶ backend client ready in `api::sessions` (live-verified calls listed in docs/api/sessions.md); UI not wired
- [x] Newline without sending (⇧Enter) — local — src/lib/keybinds/actions.ts (KEYBIND_READONLY) ⟶ live 2026-10-16, `tests/live_backend.rs` vs private backend: Shift+Enter → composer `hello\n`, nothing sent; Enter sends and clears; IME preedit+commit (`日本`) inserts text without sending
- [ ] Cancel composer / dismiss (Esc) — local — src/app/chat/composer/hooks/use-composer-esc-cancel.ts
- [ ] Rich text editor with markdown affordances — local — src/app/chat/composer/rich-editor.tsx
- [ ] Soft focus composer (/, Shift+Enter, type-to-focus) — local — src/app/chat/composer/paste-to-focus.ts, src/lib/keybinds/composer-focus-keys.ts
- [ ] Focus composer from anywhere (⌘/Ctrl+L) — local — src/app/chat/composer/focus-chord.ts
- [~] Slash command completion — RPC commands.catalog, complete.slash — src/app/chat/composer/hooks/use-slash-completions.ts ⟶ backend client ready in `api::sessions` (live-verified calls listed in docs/api/sessions.md); UI not wired
- [~] Execute a slash command — RPC slash.exec — src/app/session/hooks/use-prompt-actions/slash.ts ⟶ slash.exec/command.dispatch routing plus local model/reasoning/new/stop. Aliases remain unsupported; model-generated skill execution is not part of the no-credentials check.
- [~] @-mention path completion — RPC complete.path — src/app/chat/composer/hooks/use-at-completions.ts ⟶ backend client ready in `api::sessions` (live-verified calls listed in docs/api/sessions.md); UI not wired
- [ ] Emoji completion (:name) — local — src/app/chat/composer/hooks/use-emoji-completions.ts
- [ ] Slash help hint (?) — local — src/app/chat/composer/help-hint.tsx
- [ ] Composer input history (↑/↓) — local — src/store/composer-input-history.ts
- [ ] Undo/redo history in composer — local — src/app/chat/composer/undo-history.ts, hooks/use-composer-undo.ts
- [~] Attach image (paste / drop / picker) — RPC image.attach, image.attach_bytes — src/app/chat/composer/attachments.tsx ⟶ Ctrl+V paste done: desktop reads wl-paste/xclip, uploads with image.attach_bytes (falls back to clipboard.paste); live 2026-10-17, tests/live_attachments.rs vs private backend (image.attach_bytes → count 1, image.detach → count 0). Drop and file picker not yet; the wl-paste read itself needs a real session (not run headlessly)
- [x] Detach an image attachment — RPC image.detach — src/app/chat/hooks/use-composer-actions.ts ⟶ click the 📎 chip; live 2026-10-17, tests/live_attachments.rs vs private backend (image.attach_bytes → count 1, image.detach → count 0)
- [ ] Attach file — RPC file.attach — src/app/session/hooks/use-prompt-actions/index.ts
- [ ] Attach PDF — RPC pdf.attach — src/lib/desktop-slash-commands.ts
- [~] Large-paste collapse to attachment — RPC paste.collapse — src/app/chat/composer/large-paste.tsx ⟶ backend client ready in `api::sessions` (live-verified calls listed in docs/api/sessions.md); UI not wired
- [ ] Drag-and-drop file onto chat — local drop zone — src/app/chat/hooks/use-file-drop-zone.ts
- [ ] Drag-over affordance overlay — local — src/app/chat/chat-drop-overlay.tsx
- [ ] Screenshot attach from OS shortcut — local (Electron) + RPC image.attach_bytes — src/app/chat/composer/hooks/use-composer-screenshot.ts
- [~] Inline references (/file, @path) in composer — local + RPC complete.path — src/app/chat/composer/inline-refs.ts, path-refs.ts ⟶ backend client ready in `api::sessions` (live-verified calls listed in docs/api/sessions.md); UI not wired
- [ ] URL reference chips — local — src/app/chat/composer/url-refs.ts, url-dialog.tsx
- [x] Model pill (per-composer model switcher) — REST /api/model/set, /api/model/options — src/app/chat/composer/model-pill.tsx ⟶ live 2026-10-16, `tests/live_backend.rs` vs private backend: mid-session deepseek-v4.1-flash → glm-5.3-flash via `config.set model` (session-scoped); label updates; next reply answered by the new model. Re-verified 2026-10-16 (PR #3): a pick on the **new-chat screen** is kept and sent with `session.create` (`model`/`provider`, as the Electron app does), because `config.set model` answers 4001 without a live session. This was a real bug: the pick was silently ignored. Then switching back mid-session also works
- [x] Reasoning-effort pill — RPC config.set (agent.reasoning_effort) — src/app/chat/composer/reasoning-pill.tsx ⟶ live private backend, tests/live_backend.rs daily: actual chip/max clicks, session.create preserves max, live change reads back high, config default remains medium, switching restores low.
- [ ] Reasoning level up/down hotkeys — local — src/lib/keybinds/actions.ts
- [ ] Composer scope selector (profile/agent routing) — local — src/app/chat/composer/scope.tsx
- [ ] Micro-actions row under composer — local — src/app/chat/composer/micro-actions.tsx
- [ ] Suggestion pills (follow-up prompts) — local — src/app/chat/composer/suggestion-pills.tsx
- [ ] Composer pop-out (detach into floating window) — local (Electron) — src/store/composer-popout.ts, src/app/chat/composer/floating-surface.tsx
- [ ] Restored-draft notice — local — src/app/chat/composer/restored-draft-notice.tsx
- [x] Empty-chat intro splash — local — src/app/chat/intro-visibility.ts, src/components/assistant-ui/chat-empty-slot.tsx ⟶ serif greeting + centred composer, rs-*-empty.png
- [~] Voice input button (mic record) — REST /api/audio/transcribe — src/app/chat/composer/start-voice-button.tsx ⟶ backend client ready in `api::voice` (live-verified calls listed in docs/api/voice.md); UI not wired
- [~] Voice conversation toggle — RPC voice.toggle + REST audio — src/app/chat/composer/hooks/use-voice-conversation.ts ⟶ backend client ready in `api::voice` (live-verified calls listed in docs/api/voice.md); UI not wired
- [ ] Barge-in / stop-word while speaking — local — src/lib/voice-barge-in.ts, src/lib/voice-stop-word.ts
- [ ] Voice activity indicator — local — src/app/chat/composer/voice-activity.tsx
- [ ] Completion/thinking sound cues — local — src/lib/completion-sound.ts, src/lib/thinking-sound.ts
- [~] Auto-speak replies — local pref + RPC voice.tts — src/app/chat/composer/hooks/use-auto-speak-replies.ts ⟶ backend client ready in `api::voice` (live-verified calls listed in docs/api/voice.md); UI not wired
- [~] Session goal control row — RPC session.control — src/app/chat/composer/status-stack/session-control-goal.tsx ⟶ backend client ready in `api::sessions` (live-verified calls listed in docs/api/sessions.md); UI not wired
- [~] Heartbeat / loop session controls — RPC session.control — src/app/chat/composer/status-stack/session-control-heartbeat.tsx, session-control-loop.tsx ⟶ backend client ready in `api::sessions` (live-verified calls listed in docs/api/sessions.md); UI not wired
- [ ] Coding-status row (auto-coding indicator) — local — src/app/chat/composer/status-stack/coding-row.tsx
- [ ] Preview-status row in composer — local — src/app/chat/composer/status-stack/preview-row.tsx
- [ ] Status drawer (expandable status stack) — local — src/app/chat/composer/status-stack/drawer.tsx
- [ ] Agent-actions / session menu in composer — local — src/store/composer-actions.ts

## Transcript rendering

- [x] Streaming assistant message render — WS message.delta / message.complete — src/components/assistant-ui/thread/assistant-message.tsx ⟶ coalesced ≤1 patch/frame, row-level updates
- [ ] User message bubble + edit composer — local — src/components/assistant-ui/thread/user-message.tsx, user-edit-composer.tsx
- [ ] System message rows — local — src/components/assistant-ui/thread/system-message.tsx
- [ ] Message parts renderer (text/tool/reasoning parts) — local — src/components/assistant-ui/thread/message-parts.tsx
- [~] Markdown text rendering — local — src/components/assistant-ui/markdown-text.tsx, src/lib/markdown-preprocess.ts ⟶ hermes-core::styled rewrites headings, quotes, rules, task lists, images, HTML and code into the subset Slint renders, so one heading no longer turns a whole reply into raw text (rows.rs test uses the reply that failed). No syntax highlighting; not yet checked in a live screenshot (weston missing)
- [~] Markdown tables — local — src/components/assistant-ui/markdown-table.tsx, src/lib/markdown-table-widths.ts ⟶ aligned monospace grid (alignment honoured), list form when wider than 96 chars; unit-tested, not live-screenshotted
- [ ] Code blocks with syntax highlighting (Shiki) — local — src/components/chat/shiki-block.tsx, shiki-highlighter.tsx
- [ ] Inline code / diff rendering — local — src/components/chat/diff-lines.tsx, syntax-diff.tsx
- [ ] ANSI-colored terminal text — local — src/components/assistant-ui/ansi-text.tsx, src/lib/ansi.ts
- [x] Reasoning / thinking blocks (collapsible) — WS reasoning.delta, reasoning.available — src/lib/reasoning-blocks.ts, src/store/reasoning-disclosure.ts ⟶ Thought process block, Normal/Thinking/Verbose modes
- [ ] Reasoning step rows — local — src/lib/reasoning-step.ts
- [x] Turn activity / tool ticker — WS tool.start/tool.complete — src/components/assistant-ui/thread/turn-activity.ts ⟶ grouped "Ran N commands" rows with spinner
- [ ] Run summary (per-turn tool digest) — local — src/components/assistant-ui/tool/run-summary.ts
- [~] Changed-files card — REST /api/git + local diff — src/components/assistant-ui/thread/changed-files-card.tsx ⟶ backend client ready in `api::workspace` (live-verified calls listed in docs/api/workspace.md); UI not wired
- [ ] Thread timeline rail (jump-to-turn) — local — src/components/assistant-ui/thread/timeline.tsx, timeline-rail.tsx
- [ ] Timeline timestamps + hover time — local — src/components/assistant-ui/thread/timeline-timestamp.tsx, message-hover-time.tsx
- [~] Message reactions (emoji) — RPC message.react + WS message.reaction — src/components/assistant-ui/thread/message-reactions.tsx, src/store/reactions.ts ⟶ backend client ready in `api::sessions` (live-verified calls listed in docs/api/sessions.md); UI not wired
- [ ] Vibe hearts celebration — local — src/components/chat/vibe-hearts.tsx, src/store/vibe-hearts-enabled.ts
- [ ] Transcript directives (custom blocks) — local — src/components/assistant-ui/transcript-directive.tsx, src/lib/transcript-directives.ts
- [ ] Artifact card inline in transcript — local — src/components/assistant-ui/artifact-card.tsx
- [ ] Inline file preview directive — local — src/components/assistant-ui/inline-preview-directive.tsx
- [ ] Generated image result + zoom — RPC image.generate result — src/components/chat/generated-image-result.tsx, zoomable-image.tsx
- [ ] Transcript video playback — local — src/components/chat/transcript-video.tsx
- [ ] Mermaid diagram embeds — local — src/components/assistant-ui/embeds/mermaid-embed.tsx
- [ ] Rich link embeds (YouTube/X/Spotify/Instagram/TikTok/Pinterest/Vimeo/Maps) — local (consent-gated) — src/components/assistant-ui/embeds/providers/*
- [ ] Embed consent prompt — local — src/components/assistant-ui/embeds/embed-consent.tsx, src/store/embed-consent.ts
- [ ] SVG / listing / frame embeds — local — src/components/assistant-ui/embeds/svg-embed.tsx, listing-embed.tsx, frame-embed.tsx
- [ ] Interrupted-reply banner + resume — local — src/app/session/hooks/use-message-stream/interrupted-reply.tsx
- [~] Transcript windowing / retention (perf) — local — src/app/chat/transcript-window.ts, transcript-retention.ts ⟶ windowing verified live 2026-10-16 (600-message session, `tests/live_backend.rs` long section): 6–7 `MessageRow` instances alive while sweeping all 600 rows, RSS 27.0 → 27.1 → 27.1 MiB over two full sweeps; release binary on the hidden display: idle 0 % CPU, sweep 12 % (1×) / 24 % (2×), RSS flat (docs/PERF.md). No retention cap (all rows' data stays in the model)
- [ ] Transcript scroll restore + scroll-to-bottom — local — src/store/thread-scroll.ts, src/app/chat/scroll-to-bottom-button.tsx
- [ ] Find in transcript (⌘F, ⌘G/⌘⇧G) — local — src/lib/find-in-page.ts, src/store/find-in-page.ts
- [ ] Selection-to-composer context (⌘/Ctrl+L on selection) — local — src/lib/keybinds/actions.ts (view.selectionToComposer)
- [ ] Transcript text scale — local — src/store/chat-text-scale.ts
- [ ] Copy message / selection color handling — local — src/lib/selection-copy-colors.ts
- [x] Assistant-ui thread list + virtualization — local — src/components/assistant-ui/thread/list.tsx ⟶ Slint `ListView` instantiates only visible rows; verified live 2026-10-16 on a 600-message session (≤7 rows alive, memory flat; see windowing item)
- [ ] Message render error boundary — local — src/components/assistant-ui/message-render-boundary.tsx
- [ ] Timeline index/history (jump history) — local — src/components/assistant-ui/thread/use-timeline-history.ts

## Tools / terminal rows

- [x] Generic tool-call row (start → output → complete) — WS tool.start/tool.output/tool.complete — src/components/assistant-ui/tool/* ⟶ expand/collapse with eased height + chevron rotation
- [ ] Tool result summary + metadata — local — src/lib/tool-result-summary.ts, tool-result-metadata.ts
- [ ] Tool view mode (product vs technical) — local — src/store/tool-view.ts
- [ ] Terminal output block in transcript — local — src/components/chat/terminal-output.tsx
- [ ] ANSI stream rendering in tool rows — local — src/components/assistant-ui/ansi-text.tsx
- [~] Tool generating/in-progress shimmer — WS tool.generating — src/components/chat/status-pending-icon.tsx ⟶ status line shimmer; per-row shimmer on running group
- [ ] Tool dismiss / collapse — local — src/store/tool-dismiss.ts
- [ ] Tool drafting (in-progress args) — local — src/store/tool-drafting.ts
- [ ] Tool activity store + risk surfacing — WS tool.output_risk — src/store/tool-activity.ts
- [ ] Delegate (subagent) tool card — WS subagent.* — src/components/assistant-ui/tool/delegate.tsx
- [ ] Fallback-model tool card (provider fallback) — local — src/components/assistant-ui/tool/fallback.tsx, fallback-model/*
- [ ] Skill-activity tool row — local — src/components/assistant-ui/tool/skill-activity.ts
- [ ] MCP setup tool card — local — src/components/assistant-ui/mcp-setup-tool.tsx
- [ ] Catalog install tool card — local — src/components/assistant-ui/catalog-install-tool.tsx
- [ ] Connector tool / directory cards — RPC connectors.* — src/components/assistant-ui/connector-tool.tsx, connector-directory-card.tsx
- [ ] Start-chat tool (agent-spawned chat) — RPC session.create — src/components/assistant-ui/start-chat-tool.tsx
- [ ] Agent delivery status rows — local — src/components/assistant-ui/thread/agent-delivery.tsx
- [ ] Tool run continuity / collapse duplicate finals — local — src/lib/tool-run-continuity.ts, session/hooks/use-message-stream/collapse-duplicate-final.ts
- [ ] Tool render class mapping (per-tool renderer choice) — local — src/lib/tool-render-class.ts
- [ ] Run ticker (elapsed time) — local — src/components/assistant-ui/tool/run-ticker.tsx
- [x] Tool disclosure/expandable blocks — local — src/components/chat/disclosure-row.tsx, expandable-block.tsx ⟶ ToolGroup in chat.slint
- [ ] JSON document editor for tool args — local — src/components/chat/json-document-editor.tsx

## Approvals / clarify

- [x] Approval request card (dangerous command) — SRV approval — src/components/assistant-ui/tool/approval.tsx ⟶ live 2026-10-16, `tests/live_backend.rs` vs private backend: `rm -r <scratch dir>` raised the card; Deny → dir kept, agent reports denial; Allow once → dir removed, agent reports success; card closes, no spinner. Needed `client.capabilities {server_requests:true}` (fixed) and `request.cancel` handling (added). Card shows only the choices the backend offers (`choices` / `allow_session` / `allow_permanent`, unit-tested); screenshot docs/screenshots/rs-dark-approval-card.png
- [ ] Approval activity feed — WS approval.* — src/components/assistant-ui/tool/approval-activity.ts
- [x] Respond to approval (allow/deny/always) — RPC approval.respond / approval.received — src/lib/keybinds/approval-keys.ts ⟶ live 2026-10-16, `tests/live_backend.rs` (14/14) vs private backend: Deny/Allow once by click; **Esc denies** and **Enter (empty composer) allows once**, same keys as approval-keys.ts; **Allow for session** → next `rm -r` in that chat runs with no card, a new chat asks again; **Always allow** → `command_allowlist` written to the private config.yaml, a new chat runs it with no card
- [ ] Approval mode selector (manual/smart/off) — config approvals.mode (config.set) — src/app/shell/approval-mode-menu.tsx
- [~] Pending approvals list — RPC approval.pending — src/store/approval-mode.ts ⟶ backend client ready in `api::sessions` (live-verified calls listed in docs/api/sessions.md); UI not wired
- [ ] Clarify questions (1–5) render + submit — SRV clarify — src/components/assistant-ui/clarify/index.tsx
- [ ] Clarify choice rows / confirm bar — local — src/components/assistant-ui/clarify/core/choice-row.tsx, confirm-bar.tsx
- [ ] Clarify keyboard handlers (1–9, Enter) — local — src/components/assistant-ui/clarify/core/use-clarify-keys.ts
- [ ] Clarify pending/settled states — local — src/components/assistant-ui/clarify/pending.tsx, settled.tsx
- [ ] Setup-clarify (onboarding picks) — SRV setup_choose — src/components/assistant-ui/clarify/setup-pending.tsx, setup-rows.ts
- [ ] Undelivered clarify notice — local — src/components/assistant-ui/clarify/undelivered-notice.tsx
- [ ] Restore pending approval/clarify on resume — local — src/app/session/hooks/use-session-actions/restore-pending-approval.ts, restore-pending-clarify.ts
- [ ] Secret request (masked value) — SRV secret — src/app/session/hooks/use-message-stream/gateway-event/input-requests.ts
- [ ] Sudo request (terminal/display) — SRV sudo, display.install.sudo — src/app/session/hooks/use-message-stream/gateway-event/input-requests.ts
- [ ] Connection request (remote OAuth/pairing) — SRV connection.request, RPC connection.respond — src/store/connection-request.ts
- [ ] Free-tier challenge prompt — WS free_tier.challenge — src/store/free-tier-challenge.ts
- [~] Clarify lock (single-flight) — RPC clarify.lock — src/store/clarify.ts ⟶ backend client ready in `api::sessions` (live-verified calls listed in docs/api/sessions.md); UI not wired

## Sessions & sidebar

- [~] Session list (paged, virtualized) — REST /api/profiles/sessions — src/app/chat/sidebar/sessions-section.tsx, virtual-session-list.tsx ⟶ RPC session.list (200) grouped Today/Yesterday/7 days/Older; not paged
- [~] Sidebar recents feed — REST /api/profiles/sessions/sidebar — src/app/chat/sidebar/sessions-section.tsx ⟶ backend client ready in `api::sessions` (live-verified calls listed in docs/api/sessions.md); UI not wired
- [x] Open session / transcript hydrate — RPC session.history, REST /api/sessions/{id}/messages — src/app/session/hooks/use-session-actions/index.ts ⟶ live 2026-10-16, `tests/live_backend.rs` `sessions` section vs private backend (3 seeded chats): clicking a sidebar chat shows its history and only its (no rows from other chats), highlights it and sets the title; cold open 1.6 s, reopening a chat already live in the backend ~0.1 s (`session.resume` reuses it). Continuing an old chat: reply lands under its history and is still there after switching away and back. Switching away mid-stream: the other chat stays free of the stream and idle; coming back shows the finished reply. Uses `session.resume` (full history in one response), not the REST prefetch
- [x] New session (⌘N) — RPC session.create — src/app/session/hooks/use-session-actions/index.ts ⟶ live 2026-10-16, `tests/live_backend.rs` `sessions` section vs private backend (3 seeded chats): Ctrl+N from an open chat → empty transcript, no highlight, title "New chat"; the session is created lazily on first send
- [ ] New session in tab (⌘T) — local + RPC session.create — src/app/chat/new-session-drag.ts
- [ ] New window (⌘⇧N) — local (Electron session-windows) — electron/session-windows.ts
- [x] Switch to next/prev session (⌃Tab / ⌃⇧Tab) — local — src/store/session-switcher.ts ⟶ live 2026-10-16, `tests/live_backend.rs` `sessions` section vs private backend (3 seeded chats): walks the sidebar order (this app has no tabs, unlike the Electron recent-tab switcher), wrapping at both ends; from a new chat ⌃Tab opens the first chat, ⌃⇧Tab the last. Order logic unit-tested (`sessions::tests::cycling`)
- [ ] Jump to session slot N (⌃1…⌃9) — local — src/lib/keybinds/actions.ts
- [~] Close session — RPC session.close — src/app/session/hooks/use-session-actions/index.ts ⟶ backend client ready in `api::sessions` (live-verified calls listed in docs/api/sessions.md); UI not wired
- [ ] Delete session — RPC session.delete — src/store/session-removal.ts
- [~] Archive session — RPC session.archive — src/app/chat/sidebar/session-actions-menu.tsx ⟶ backend client ready in `api::sessions` (live-verified calls listed in docs/api/sessions.md); UI not wired
- [~] Pin/unpin session — local + REST /api/sessions/{id} — src/store/session-pin-sync.ts ⟶ backend client ready in `api::sessions` (live-verified calls listed in docs/api/sessions.md); UI not wired
- [ ] Rename session / auto-title — RPC session.title — src/app/chat/sidebar/session-actions-menu.tsx
- [~] Hide session — RPC session.set_hidden — src/store/session-list-density.ts ⟶ backend client ready in `api::sessions` (live-verified calls listed in docs/api/sessions.md); UI not wired
- [~] Session read/unread state — REST PATCH /api/sessions/{id} — src/store/session-unread-remote.ts ⟶ backend client ready in `api::sessions` (live-verified calls listed in docs/api/sessions.md); UI not wired
- [~] Session search (⌘⇧F) — REST /api/sessions/search — src/app/chat/sidebar/use-server-session-search.ts, src/lib/session-search.ts ⟶ backend client ready in `api::sessions` (live-verified calls listed in docs/api/sessions.md); UI not wired
- [ ] Session sort (recent/title) — local — src/store/sidebar-sort.ts
- [ ] Session list density (compact/comfortable/detailed) — local — src/store/session-list-density.ts
- [ ] Sidebar grouping cycle — local — src/app/chat/sidebar/order.ts
- [ ] Sidebar collapse/expand (⌘B) — local — src/store/layout.ts
- [ ] Sidebar filter menu — local — src/app/chat/sidebar/filter-menu.tsx
- [ ] Session color dots + status dots — local — src/store/session-color.ts, session-dot-state.ts
- [ ] Session tile (multi-pane tile view) — local — src/app/chat/session-tile.tsx, session-tile-actions.ts
- [ ] Session tile grid / route tiles — local — src/store/route-tiles.ts, src/app/chat/route-tile.tsx
- [ ] Pane split / mirror (multi-session layout) — local — src/app/chat/pane-mirror.tsx, src/components/pane-shell/*
- [~] Fleet rail (multi-gateway session roster) — RPC session.active_list — src/app/chat/sidebar/fleet-rail.ts, use-fleet-roster.ts ⟶ backend client ready in `api::sessions` (live-verified calls listed in docs/api/sessions.md); UI not wired
- [ ] Gateway/profile grouping in sidebar — local — src/app/chat/sidebar/gateway-groups.tsx
- [ ] Session drag-and-drop reorder — local — src/app/chat/sidebar/reorderable-list.tsx, session-drag.ts
- [ ] Load-more row for long session lists — local — src/app/chat/sidebar/load-more-row.tsx
- [~] Session usage/token meter — RPC session.usage, REST /api/analytics/usage — src/app/session/hooks/use-session-actions/index.ts ⟶ backend client ready in `api::sessions` (live-verified calls listed in docs/api/sessions.md); UI not wired
- [~] Context breakdown panel — RPC session.context_breakdown — src/app/shell/hooks/use-context-breakdown.ts ⟶ backend client ready in `api::sessions` (live-verified calls listed in docs/api/sessions.md); UI not wired
- [~] Session branch (fork at message) — RPC session.branch, session.branch_stored, session.branch_whole — src/app/session/hooks/use-session-actions/branch-create-key.ts ⟶ backend client ready in `api::sessions` (live-verified calls listed in docs/api/sessions.md); UI not wired
- [~] Session compress (manual) — RPC session.compress — src/store/compaction.ts ⟶ backend client ready in `api::sessions` (live-verified calls listed in docs/api/sessions.md); UI not wired
- [~] Session undo — RPC session.undo — src/app/session/hooks/use-session-actions/index.ts ⟶ backend client ready in `api::sessions` (live-verified calls listed in docs/api/sessions.md); UI not wired
- [~] Session resume (reconnect/revive) — RPC session.resume, session.most_recent — src/app/session/hooks/use-prompt-actions/resolve-target-session.ts ⟶ backend client ready in `api::sessions` (live-verified calls listed in docs/api/sessions.md); UI not wired
- [~] Session status polling — RPC session.status — src/lib/desktop-slash-commands.ts ⟶ backend client ready in `api::sessions` (live-verified calls listed in docs/api/sessions.md); UI not wired
- [ ] Dead-session pruning — local — src/store/dead-session-prune.ts
- [ ] Session watchdog / gone latch — local — src/store/session-watchdog.ts, session-gone-latch.ts
- [ ] Broken/missing-session panes — local — src/app/retirement-view.tsx
- [ ] Backend/agent-scoped session caches — local — src/store/session-states.ts
- [~] Session unread aggregation across connections — RPC session.active_list — src/store/session-unread-remote.ts ⟶ backend client ready in `api::sessions` (live-verified calls listed in docs/api/sessions.md); UI not wired
- [ ] Resume-exhausted overlay — local — src/app/chat/resume-exhausted-overlay.tsx
- [ ] Session sidebar focus hotkey — local — src/store/session-sidebar-focus.ts

## Projects / workspaces

- [~] Projects list (groups of folders/repos/worktrees) — RPC projects.list — src/store/projects.ts ⟶ project dialog lists non-archived projects and its rows are pointer-click tested against the private backend; hierarchical grouping remains unbuilt.
- [x] Create project — RPC projects.create — src/app/chat/sidebar/project-dialog.tsx ⟶ live private backend, daily: actual Add project click, existing folder validation, missing folder rejection without creation, independent projects.list readback; dialog currently adds one folder.
- [~] Update project (name/color/icon) — RPC projects.update — src/app/chat/sidebar/projects/project-menu.tsx ⟶ name edit click and independent projects.get readback verified live; color/icon editing is not built.
- [~] Delete project — RPC projects.delete — src/app/chat/sidebar/projects/project-menu.tsx ⟶ backend client ready in `api::workspace` (live-verified calls listed in docs/api/workspace.md); UI not wired
- [~] Archive project — RPC projects.archive — src/store/projects.ts ⟶ backend client ready in `api::workspace` (live-verified calls listed in docs/api/workspace.md); UI not wired
- [~] Enter/leave project (sidebar scope) — RPC projects.set_active — src/app/chat/sidebar/projects/workspace-groups.ts ⟶ actual selection/leave clicks, pointer readback, correct session.create cwd and restore/new-chat inheritance verified live; Recents remains global, not project-filtered.
- [~] Set primary project — RPC projects.set_primary — src/store/projects.ts ⟶ backend client ready in `api::workspace` (live-verified calls listed in docs/api/workspace.md); UI not wired
- [~] Add folder to project — RPC projects.add_folder — src/app/chat/sidebar/projects/project-menu.tsx ⟶ backend client ready in `api::workspace` (live-verified calls listed in docs/api/workspace.md); UI not wired
- [~] Remove folder from project — RPC projects.remove_folder — src/app/chat/sidebar/projects/project-menu.tsx ⟶ backend client ready in `api::workspace` (live-verified calls listed in docs/api/workspace.md); UI not wired
- [~] Project tree (folders/repos) — RPC projects.tree — src/store/project-tree.ts ⟶ backend client ready in `api::workspace` (live-verified calls listed in docs/api/workspace.md); UI not wired
- [~] Project facts (stack/commands summary) — RPC project.facts — src/store/projects.ts ⟶ backend client ready in `api::workspace` (live-verified calls listed in docs/api/workspace.md); UI not wired
- [~] Project sessions — RPC projects.project_sessions — src/app/chat/sidebar/use-entered-project-sessions.ts ⟶ backend client ready in `api::workspace` (live-verified calls listed in docs/api/workspace.md); UI not wired
- [~] Project for current cwd — RPC projects.for_cwd — src/store/project-scope.ts ⟶ backend client ready in `api::workspace` (live-verified calls listed in docs/api/workspace.md); UI not wired
- [~] Auto repository discovery (repo scan) — RPC projects.discover_repos — src/store/projects.ts ⟶ backend client ready in `api::workspace` (live-verified calls listed in docs/api/workspace.md); UI not wired
- [ ] Repo scan roots / exclusions — config desktop.repo_scan_* — src/app/settings/constants.ts
- [~] Record discovered repos — RPC projects.record_repos — src/store/projects.ts ⟶ backend client ready in `api::workspace` (live-verified calls listed in docs/api/workspace.md); UI not wired
- [ ] New git worktree (⌘⇧B) — local (Electron git-worktree) — electron/git-worktree-ops.ts
- [ ] Base-branch picker for worktree — local — src/app/chat/sidebar/projects/base-branch-picker.tsx
- [ ] Worktree dialog — local — src/app/chat/sidebar/projects/worktree-dialog.tsx
- [~] Open folder as project (⌘O) — local (Electron select-paths) — electron/select-paths-dialog.ts ⟶ the project dialog accepts a full existing-folder path; native file dialog and Ctrl+O are not built.
- [~] Change workspace cwd — RPC session.workspace.move, session.cwd.set — src/app/session/hooks/use-cwd-actions.ts ⟶ backend client ready in `api::sessions` (live-verified calls listed in docs/api/sessions.md); UI not wired
- [ ] Project appearance (color/icon) — local — src/app/chat/sidebar/projects/project-appearance.tsx
- [ ] Project filter in sidebar — local — src/app/chat/sidebar/project-filter.ts
- [ ] Workspace page header (page-owned controls) — local — src/app/contrib/workspace-page-header.tsx
- [ ] Sidebar workspace nav (Agents/Capabilities/Messaging/Artifacts/Cron) — local routes — src/app/routes.ts

## Models & providers

- [ ] Main model selector / settings — REST /api/model/set, /api/model/options, /api/model/info — src/app/settings/model-settings.tsx
- [ ] Model context-length override — config model_context_length — src/app/settings/constants.ts
- [ ] Auxiliary (task) model rows — REST /api/model/auxiliary — src/app/settings/aux-task-rows.tsx
- [ ] MoA (mixture-of-agents) models — REST /api/model/moa — src/app/settings/model-settings.tsx
- [ ] Fallback models — REST /api/model/options + config fallback_providers — src/app/settings/fallback-models-field.tsx
- [ ] Recommended default per provider — REST /api/model/recommended-default — src/app/settings/model-settings.tsx
- [ ] Provider accounts view (OAuth sign-in) — REST /api/providers/oauth/* — src/app/settings/providers-settings.tsx
- [~] Provider API keys view — REST /api/env, /api/env/reveal — src/app/settings/keys-settings.tsx, env-credentials.tsx ⟶ backend client ready in `api::system` (live-verified calls listed in docs/api/system.md); UI not wired
- [ ] Custom endpoints (OpenAI/Anthropic-compatible) — REST /api/providers/custom-endpoints — src/app/settings/custom-endpoints-settings.tsx
- [ ] Validate provider/endpoint — REST /api/providers/validate, /api/providers/custom-endpoints/validate — src/app/settings/helpers.ts
- [ ] Favorite models — local — src/store/favorite-models.ts
- [ ] Model visibility (hide models) — local — src/store/model-visibility.ts, src/app/model-visibility-overlay.tsx
- [ ] Model picker overlay (⌘⇧M) — REST /api/model/options — src/app/model-picker-overlay.tsx
- [ ] Model presets — local — src/store/model-presets.ts
- [ ] Model pricing display — local — src/store/model-pricing.ts
- [ ] Custom models (user-added) — local + REST /api/model/options — src/store/custom-models.ts
- [ ] Per-profile model assignment — local — src/store/model-assignment.ts
- [ ] Model menu (titlebar model switcher) — local — src/app/shell/model-menu-panel.tsx, use-model-menu-controller.ts
- [ ] Model catalog menu — local — src/app/shell/model-catalog-menu.tsx
- [ ] Disconnect a model/provider — RPC model.disconnect — src/hermes.ts
- [ ] Provider wait (in-flight model selection) — local — src/store/provider-wait.ts
- [~] Guarded model switch (warn mid-turn) — local — src/lib/guarded-model-switch.ts ⟶ deferred model changes and backend confirmation warnings are surfaced. Expensive-model confirmation UI remains unbuilt; no automatic confirmation.
- [ ] Local models (hardware/runtime/download) — REST /api/local-models/* — src/app/settings/local-models-settings.tsx
- [ ] Local model quickstart — REST /api/local-models/quickstart — src/app/settings/local-models-quickstart.tsx
- [ ] Local model runtime install — REST /api/local-models/runtime/install — src/app/settings/local-models-runtime-section.tsx
- [ ] Local model download/pause/resume — REST /api/local-models/download, jobs — src/app/settings/local-model-download-progress.tsx
- [ ] Local model activate/eject — REST /api/local-models/activate, /eject — src/app/settings/local-models-actions.ts
- [ ] Local model search + sideload (HF) — REST /api/local-models/search, /sideload — src/app/settings/local-models-browse.tsx
- [ ] Local models feature flag — local — src/store/local-models-flag.ts
- [ ] Service tier / API retries / max turns — config agent.* — src/app/settings/constants.ts

## Profiles

- [x] Profiles list — RPC profiles.list, REST /api/profiles — src/app/profiles/index.tsx ⟶ live 2026-10-17, `tests/live_backend.rs` `capabilities` section vs private backend (pointer clicks + independent REST read-back): Capabilities › Profiles lists default/swe/ea
- [x] Create profile — RPC profiles.create — src/app/profiles/create-profile-dialog.tsx ⟶ live 2026-10-17, `tests/live_backend.rs` `capabilities` section vs private backend (pointer clicks + independent REST read-back): Create profile → GET /api/profiles has it; via REST POST /api/profiles (as `src/api/profiles.ts`)
- [x] Rename profile — REST /api/profiles/{name} (PATCH) — src/app/profiles/rename-profile-dialog.tsx ⟶ live 2026-10-17, `tests/live_backend.rs` `capabilities` section vs private backend (pointer clicks + independent REST read-back): Rename → list shows only the new name
- [x] Delete profile — RPC/REST profile delete — src/app/profiles/delete-profile-dialog.tsx ⟶ live 2026-10-17, `tests/live_backend.rs` `capabilities` section vs private backend (pointer clicks + independent REST read-back): first click arms "Confirm delete", second deletes; backend read-back gone; default profile has no Delete
- [~] Describe/configure profile — RPC profiles.describe, profiles.configure — src/plugins/hermes-bots/data.ts ⟶ backend client ready in `api::extras` (live-verified calls listed in docs/api/extras.md / extras-admin.md); UI not wired
- [x] Profile soul file — REST /api/profiles/{name}/soul — src/plugins/hermes-bots/soul.ts ⟶ live 2026-10-17, `tests/live_backend.rs` `capabilities` section vs private backend (pointer clicks + independent REST read-back): SOUL.md editor save → GET /api/profiles/{name}/soul matches
- [~] Profile assets (avatar) — RPC profiles.get_asset, profiles.set_asset — src/components/assistant-ui/thread/user-message.tsx ⟶ backend client ready in `api::extras` (live-verified calls listed in docs/api/extras.md / extras-admin.md); UI not wired
- [~] Profile export/import — REST /api/profiles/{name}/export, /api/profiles/import — src/api/profiles.ts ⟶ backend client ready in `api::extras` (live-verified calls listed in docs/api/extras.md / extras-admin.md); UI not wired
- [ ] Switch profile (⌘D default; ⌘1…⌘9 / ⌘⌥1…⌘⌥9 named) — local + connection route — src/store/profile.ts
- [ ] Next/prev profile (⌘⇧] / ⌘⇧[) — local — src/lib/keybinds/actions.ts
- [ ] Toggle all profiles (⌘⇧0) — local — src/lib/keybinds/actions.ts
- [ ] Create profile hotkey (unbound) — local — src/lib/keybinds/actions.ts
- [ ] Profile rail / dropdown switcher — local — src/app/chat/sidebar/profile-switcher.tsx, profile-dropdown-switcher.tsx
- [ ] Profile scope selector (settings apply-to) — local — src/app/settings/profile-scope.tsx, src/store/settings-scope.ts
- [ ] Per-profile remote override — local — src/store/profile-remote-override.ts, profile-remote-override-dialog.tsx
- [ ] Profile launch menu — local — src/app/chat/sidebar/profile-launch-menu.tsx
- [ ] Profile share (soul/config bundle) — local — src/store/profile-share.ts
- [~] Profile setup command — REST /api/profiles/{name}/setup-command — src/api/profiles.ts ⟶ backend client ready in `api::extras` (live-verified calls listed in docs/api/extras.md / extras-admin.md); UI not wired
- [ ] Profile cache / prewarm — local — src/store/profile-cache.ts, use-profile-prewarm.ts
- [ ] Default profile preference — local — src/store/default-profile.ts
- [ ] Profile color / short label / order — local — src/lib/profile-color.ts, profile-order.ts
- [ ] Real-profile consent (agent memory of user) — local — src/store/real-profile-consent.ts

## Skills

- [x] Skills list — REST /api/skills — src/app/capabilities/skills/skills-tab.tsx ⟶ live 2026-10-17, `tests/live_backend.rs` `capabilities` section vs private backend (pointer clicks + independent REST read-back): Capabilities › Skills, with a client-side filter
- [x] Skill detail + SKILL.md viewer — REST /api/skills/content — src/app/capabilities/skills/skill-detail.tsx ⟶ live 2026-10-17, `tests/live_backend.rs` `capabilities` section vs private backend (pointer clicks + independent REST read-back): View opens SKILL.md (frontmatter shown); the pane also saves edits via PUT /api/skills/content (edit save not live-verified)
- [x] Toggle skill enabled — REST /api/skills/toggle — src/app/capabilities/skills/skills-data.ts ⟶ live 2026-10-17, `tests/live_backend.rs` `capabilities` section vs private backend (pointer clicks + independent REST read-back): switch off/on read back from GET /api/skills; in the swe pill only swe changes
- [~] Skill Hub (official source) — REST /api/skills/hub/official, /sources — src/app/capabilities/skills/embedded-hub-picker.tsx ⟶ backend client ready in `api::extras` (live-verified calls listed in docs/api/extras.md / extras-admin.md); UI not wired
- [~] Skill Hub search — REST /api/skills/hub/search — src/app/capabilities/skills/skills-data.ts ⟶ backend client ready in `api::extras` (live-verified calls listed in docs/api/extras.md / extras-admin.md); UI not wired
- [~] Skill Hub preview — REST /api/skills/hub/preview — src/app/capabilities/skills/official-skill-detail.tsx ⟶ backend client ready in `api::extras` (live-verified calls listed in docs/api/extras.md / extras-admin.md); UI not wired
- [~] Skill Hub security scan — REST /api/skills/hub/scan — src/app/capabilities/skills/skills-data.ts ⟶ backend client ready in `api::extras` (live-verified calls listed in docs/api/extras.md / extras-admin.md); UI not wired
- [~] Skill install/uninstall/update — REST /api/skills/hub/install|uninstall|update — src/app/capabilities/skills/skills-data.ts ⟶ backend client ready in `api::extras` (live-verified calls listed in docs/api/extras.md / extras-admin.md); UI not wired
- [ ] Skill provenance display — local — src/app/capabilities/skills/skill-provenance.ts
- [ ] Skill frontmatter parse — local — src/app/capabilities/skills/frontmatter.ts
- [ ] Skill deeplink install — local + REST install — src/store/skill-deeplink-install.ts
- [~] Skill manage (create/edit/delete) — RPC skills.manage — src/plugins/hermes-bots/skills-hub.tsx ⟶ create via REST POST /api/skills verified live (default + swe scope); edit implemented (PUT /api/skills/content) but not live-verified; delete not built
- [~] Skill reload — RPC skills.reload — src/plugins/hermes-bots/skills-hub.tsx ⟶ backend client ready in `api::extras` (live-verified calls listed in docs/api/extras.md / extras-admin.md); UI not wired
- [ ] Skill suggestions in composer — local — src/store/suggestion-providers/skill.ts
- [x] Skill curator status — REST /api/curator — src/api/system.ts ⟶ live 2026-10-17, `tests/live_backend.rs` `capabilities` section vs private backend (pointer clicks + independent REST read-back): Memory tab curator row shows interval/last run; paused flag read back
- [~] Background skill-curator control (pause/run) — REST /api/curator/paused, /run — src/api/system.ts ⟶ pause/resume switch verified live (GET /api/curator paused true→false); Run now implemented behind a confirm click (explicit ?profile=) but not run live: it starts an agent run

## Tools / toolsets

- [x] Toolsets list — REST /api/tools/toolsets — src/app/capabilities/toolsets/toolsets-tab.tsx ⟶ live 2026-10-17, `tests/live_backend.rs` `capabilities` section vs private backend (pointer clicks + independent REST read-back): Capabilities › Tools lists toolsets with tool counts / needs-setup badge
- [~] Toolset detail — REST /api/tools/toolsets/{name} — src/app/capabilities/toolsets/toolset-detail.tsx ⟶ backend client ready in `api::extras` (live-verified calls listed in docs/api/extras.md / extras-admin.md); UI not wired
- [x] Toggle toolset enabled — REST /api/tools/toolsets/{name} (POST) — src/app/capabilities/toolsets/toolsets-data.ts ⟶ live 2026-10-17, `tests/live_backend.rs` `capabilities` section vs private backend (pointer clicks + independent REST read-back): `terminal` off/on read back from GET /api/tools/toolsets; a post-setup install start is surfaced as a notice (not exercised: test avoids install toolsets)
- [ ] Toolset config — REST /api/tools/toolsets/{name}/config — src/app/settings/toolset-config-panel.tsx
- [ ] Toolset model/provider selection — REST /api/tools/toolsets/{name}/model, /provider — src/app/settings/toolset-config-panel.tsx
- [~] Toolset model list — REST /api/tools/toolsets/{name}/models — src/app/settings/toolset-config-panel.tsx ⟶ backend client ready in `api::extras` (live-verified calls listed in docs/api/extras.md / extras-admin.md); UI not wired
- [ ] Toolset post-setup — REST /api/tools/toolsets/{name}/post-setup — src/app/settings/toolset-config-panel.tsx
- [ ] Tool calls view (per-toolset) — local — src/app/capabilities/toolsets/tool-calls.ts
- [~] Tools list (agent tools) — RPC tools.list — src/hermes.ts ⟶ backend client ready in `api::extras` (live-verified calls listed in docs/api/extras.md / extras-admin.md); UI not wired
- [~] Configure tools — RPC tools.configure — src/hermes.ts ⟶ backend client ready in `api::extras` (live-verified calls listed in docs/api/extras.md / extras-admin.md); UI not wired
- [~] Show tool schema — RPC tools.show — src/hermes.ts ⟶ backend client ready in `api::extras` (live-verified calls listed in docs/api/extras.md / extras-admin.md); UI not wired
- [ ] Toolsets config field (enabled set) — config toolsets — src/app/settings/constants.ts
- [~] Terminal execution backends (local/docker/singularity/modal/daytona/ssh) — REST /api/tools/terminal/backends|backend — src/app/settings/terminal-backend-panel.tsx ⟶ backend client ready in `api::extras` (live-verified calls listed in docs/api/extras.md / extras-admin.md); UI not wired
- [~] Computer-use status + permissions — REST /api/tools/computer-use/status|permissions/grant — src/app/settings/computer-use-panel.tsx ⟶ backend client ready in `api::extras` (live-verified calls listed in docs/api/extras.md / extras-admin.md); UI not wired
- [ ] Browser real-profile panel — local + config browser.use_real_profile — src/app/settings/browser-real-profile-panel.tsx
- [ ] Tool-use enforcement — config agent.tool_use_enforcement — src/app/settings/constants.ts
- [ ] Command allowlist — config command_allowlist — src/app/settings/constants.ts
- [ ] Tool output limits — config tool_output.* — src/app/settings/constants.ts
- [ ] Checkpoints (file snapshots) — config checkpoints.* — src/app/settings/constants.ts
- [~] Rollback list/diff/restore — RPC rollback.list|diff|restore — src/hermes.ts ⟶ backend client ready in `api::workspace` (live-verified calls listed in docs/api/workspace.md); UI not wired

## MCP

- [x] MCP servers list — REST /api/mcp/servers + RPC mcp.servers.list — src/app/capabilities/mcp/use-mcp-servers.ts ⟶ live 2026-10-17, `tests/live_backend.rs` `capabilities` section vs private backend (pointer clicks + independent REST read-back): REST /api/mcp/servers; plugin-provided servers shown read-only
- [x] Add MCP server — REST /api/mcp/servers (POST) + RPC mcp.servers.add — src/app/capabilities/mcp/mcp-editor.tsx ⟶ live 2026-10-17, `tests/live_backend.rs` `capabilities` section vs private backend (pointer clicks + independent REST read-back): stdio add saved as command `echo` args [live-test] (whitespace split, no shell); http(s) URL form unit-tested only
- [ ] Edit MCP server — REST /api/mcp/servers/{name} — src/app/capabilities/mcp/mcp-editor.tsx
- [x] Remove MCP server — RPC mcp.servers.remove — src/app/capabilities/mcp/use-mcp-servers.ts ⟶ live 2026-10-17, `tests/live_backend.rs` `capabilities` section vs private backend (pointer clicks + independent REST read-back): two-click Remove → backend read-back gone
- [x] Enable/disable MCP server — REST /api/mcp/servers/{name}/enabled — src/app/capabilities/mcp/use-mcp-servers.ts ⟶ live 2026-10-17, `tests/live_backend.rs` `capabilities` section vs private backend (pointer clicks + independent REST read-back): switch → GET enabled=false
- [~] Test MCP server — REST /api/mcp/servers/{name}/test + RPC mcp.servers.test — src/app/capabilities/mcp/use-mcp-servers.ts ⟶ implemented (POST …/test, 60 s timeout, runs off the chat loop) but not run live: it spawns/connects the server
- [~] MCP server status/health — RPC mcp.servers.status — src/store/mcp-health.ts ⟶ backend client ready in `api::extras` (live-verified calls listed in docs/api/extras.md / extras-admin.md); UI not wired
- [ ] MCP OAuth start/poll/callback/cancel — RPC mcp.servers.oauth.* — src/lib/mcp-dashboard-oauth.ts
- [~] MCP API key — RPC mcp.servers.set_api_key — src/hermes.ts ⟶ backend client ready in `api::extras` (live-verified calls listed in docs/api/extras.md / extras-admin.md); UI not wired
- [~] MCP catalog (Nous-approved) — RPC mcp.catalog, REST /api/mcp/catalog — src/app/capabilities/mcp/install-catalog-entry.ts ⟶ backend client ready in `api::extras` (live-verified calls listed in docs/api/extras.md / extras-admin.md); UI not wired
- [~] Install MCP from catalog — REST /api/mcp/catalog/install — src/app/capabilities/mcp/install-catalog-entry.ts ⟶ backend client ready in `api::extras` (live-verified calls listed in docs/api/extras.md / extras-admin.md); UI not wired
- [~] Reload MCP — RPC reload.mcp — src/app/capabilities/mcp/use-mcp-servers.ts ⟶ backend client ready in `api::plugins` (live-verified calls listed in docs/api/plugins.md); UI not wired
- [ ] MCP logs — local — src/app/capabilities/mcp/mcp-logs.tsx
- [ ] MCP tool filter — local — src/lib/mcp-tool-filter.ts
- [ ] MCP deeplink install — local + RPC — src/store/mcp-deeplink-install.ts, src/lib/mcp-deeplink.ts
- [ ] MCP brands/icons + cost estimate — local — src/lib/mcp-brands.tsx, mcp-cost.ts
- [ ] MCP suggestions in composer — local — src/store/suggestion-providers/mcp.ts
- [ ] Confirm MCP reloads (safety) — config approvals.mcp_reload_confirm — src/app/settings/constants.ts

## Plugins / connectors

- [~] Installed plugins list — REST /api/plugins + RPC plugins.list — src/app/capabilities/plugins/plugins-tab.tsx ⟶ backend client ready in `api::plugins` (live-verified calls listed in docs/api/plugins.md); UI not wired
- [~] Plugin catalog (browse/install) — RPC plugins.catalog.lookup — src/lib/plugin-catalog.ts ⟶ backend client ready in `api::plugins` (live-verified calls listed in docs/api/plugins.md); UI not wired
- [~] Install plugin from catalog — local + RPC plugins.manage — src/store/plugin-catalog-install.ts ⟶ backend client ready in `api::plugins` (live-verified calls listed in docs/api/plugins.md); UI not wired
- [~] Enable/disable/uninstall plugin — REST /api/plugins/{id} + RPC plugins.manage — src/store/agent-plugins.ts ⟶ backend client ready in `api::plugins` (live-verified calls listed in docs/api/plugins.md); UI not wired
- [ ] Plugin package/metadata — local — src/app/capabilities/plugins/plugin-packages.ts
- [ ] Plugin settings pages (per-plugin) — local — src/app/settings/plugin-settings.tsx
- [ ] Plugin settings form (declarative) — local — src/app/settings/plugin-settings-form.tsx
- [ ] Plugin install modal — local — src/app/settings/plugin-install-modal.tsx
- [ ] Plugin install request (agent-initiated) — local — src/store/plugin-install-request.ts
- [ ] Plugin contributions registry (routes/panes/statusbar/keybinds/appearance/context menu) — local — src/contrib/registry.ts, src/app/contrib/*
- [ ] Contributed routes / full pages — local — src/app/routes.ts (ROUTES_AREA)
- [ ] Contributed sidebar nav rows — local — src/app/routes.ts (SIDEBAR_NAV_AREA)
- [ ] Contributed profile-group header — local — src/app/routes.ts (SIDEBAR_PROFILE_GROUP_HEADER_AREA)
- [ ] Plugin SDK host bridge (request/state/ui/actions) — local — src/sdk/index.ts
- [ ] Bundled plugins (hello-runtime, hermes-bots, kanban, radio) — local — src/plugins/*
- [~] Connectors catalog + directory — RPC connectors.catalog — src/app/capabilities/connectors/connectors-directory.tsx ⟶ backend client ready in `api::plugins` (live-verified calls listed in docs/api/plugins.md); UI not wired
- [~] Connect a connector (OAuth) — RPC connectors.connect, connectors.operation.* — src/app/capabilities/connectors/data/mutations.ts ⟶ backend client ready in `api::plugins` (live-verified calls listed in docs/api/plugins.md); UI not wired
- [~] Connector accounts list — RPC connectors.accounts (+ remove/rename) — src/app/capabilities/connectors/accounts-section.tsx ⟶ backend client ready in `api::plugins` (live-verified calls listed in docs/api/plugins.md); UI not wired
- [~] Connector tools list + enable — RPC connectors.tools — src/app/capabilities/connectors/tools-list.tsx ⟶ backend client ready in `api::plugins` (live-verified calls listed in docs/api/plugins.md); UI not wired
- [~] Connector policy — RPC connectors.policy.get|set — src/app/capabilities/connectors/use-tools-editor.ts ⟶ backend client ready in `api::plugins` (live-verified calls listed in docs/api/plugins.md); UI not wired
- [~] Connector hosted actions — RPC connectors.accounts + hosted-dialog — src/app/capabilities/connectors/data/hosted-actions.ts ⟶ backend client ready in `api::plugins` (live-verified calls listed in docs/api/plugins.md); UI not wired
- [ ] Connector local server control — local + RPC — src/app/capabilities/connectors/local-server-control.tsx
- [ ] Connector deeplinks — local — src/app/capabilities/connectors/data/deep-link.ts
- [~] Connector suggestions in composer — RPC connectors.list — src/store/connector-catalog.ts ⟶ backend client ready in `api::plugins` (live-verified calls listed in docs/api/plugins.md); UI not wired

## Memory

- [x] Memory status (provider + file sizes) — REST /api/memory — src/app/settings/memory/connect.tsx ⟶ live 2026-10-17, `tests/live_backend.rs` `capabilities` section vs private backend (pointer clicks + independent REST read-back): MEMORY.md/USER.md sizes, providers, active badge
- [x] Memory reset — REST /api/memory/reset — src/api/system.ts ⟶ live 2026-10-17, `tests/live_backend.rs` `capabilities` section vs private backend (pointer clicks + independent REST read-back): two-click Reset deleted a fixture MEMORY.md; sends explicit ?profile= (the route 400s an unnamed profile on a multi-profile backend)
- [~] Memory providers discovery — REST /api/memory/providers/{p}/config — src/app/settings/memory/provider-config-panel.tsx ⟶ backend client ready in `api::extras` (live-verified calls listed in docs/api/extras.md / extras-admin.md); UI not wired
- [~] Memory provider connect (OAuth) — REST /api/memory/providers/{p}/oauth/start|status — src/app/settings/memory/connect.tsx ⟶ backend client ready in `api::extras` (live-verified calls listed in docs/api/extras.md / extras-admin.md); UI not wired
- [ ] Memory provider config modal — local + REST — src/app/settings/memory/provider-config-modal.tsx
- [ ] Persistent memory toggle — config memory.memory_enabled — src/app/settings/constants.ts
- [ ] User profile toggle — config memory.user_profile_enabled — src/app/settings/constants.ts
- [ ] Memory/profile char budgets — config memory.memory_char_limit, user_char_limit — src/app/settings/constants.ts
- [~] Memory provider select — config memory.provider — src/app/settings/constants.ts ⟶ Use button → PUT /api/memory/provider implemented and unit-tested; not live-verified (fixture home has no configured external provider)
- [ ] Context engine select — config context.engine — src/app/settings/constants.ts
- [ ] Auto-compression + thresholds — config compression.* — src/app/settings/constants.ts
- [ ] Compression aux-model timeout — config auxiliary.compression.timeout — src/app/settings/constants.ts
- [ ] Reasoning blocks display toggle — config display.show_reasoning — src/app/settings/constants.ts
- [ ] Memory provider troubleshooting status — local — src/app/settings/memory/*

## Messaging gateways

- [~] Messaging platforms list — REST /api/messaging/platforms — src/app/messaging/index.tsx ⟶ backend client ready in `api::messaging` (live-verified calls listed in docs/api/messaging.md); UI not wired
- [~] Platform detail/config — REST /api/messaging/platforms/{id} — src/app/messaging/index.tsx ⟶ backend client ready in `api::messaging` (live-verified calls listed in docs/api/messaging.md); UI not wired
- [ ] Test a platform — REST /api/messaging/platforms/{id}/test — src/app/messaging/index.tsx
- [~] Telegram QR onboarding — REST /api/messaging/telegram/onboarding/* — src/app/messaging/telegram-qr-setup.tsx ⟶ backend client ready in `api::messaging` (live-verified calls listed in docs/api/messaging.md); UI not wired
- [~] Pairing status — REST /api/pairing — src/api/messaging.ts ⟶ backend client ready in `api::messaging` (live-verified calls listed in docs/api/messaging.md); UI not wired
- [~] Approve pairing — REST /api/pairing/approve — src/api/messaging.ts ⟶ backend client ready in `api::messaging` (live-verified calls listed in docs/api/messaging.md); UI not wired
- [~] Revoke pairing — REST /api/pairing/revoke — src/api/messaging.ts ⟶ backend client ready in `api::messaging` (live-verified calls listed in docs/api/messaging.md); UI not wired
- [ ] Allowlist field (per-platform users) — local — src/app/messaging/allowlist-field.tsx
- [ ] Bot relay (outbox/reply/roster) — RPC bot_relay.* — src/plugins/hermes-bots/*
- [ ] Platform changed events — WS platforms.changed / pairing.changed — src/store/notifications.ts
- [ ] Bot/group chat scope — local — src/store/bot-chat-scope.ts
- [ ] Group chat orchestration — RPC groups.* — src/plugins/hermes-bots/group-turns.ts

## Scheduled / cron

- [x] Cron jobs list — REST /api/cron/jobs — src/app/cron/index.tsx ⟶ live 2026-10-17, `tests/live_backend.rs` `capabilities` section vs private backend (pointer clicks + independent REST read-back): Scheduled tab (sidebar Scheduled opens it)
- [~] Create/edit cron job — REST /api/cron/jobs (POST/PUT) + RPC cron.manage — src/app/cron/cron-actions.ts ⟶ create (name/schedule/prompt, deliver local) and prompt edit verified live; schedule/name/delivery/model editing not built
- [x] Delete cron job — REST /api/cron/jobs/{id} (DELETE) — src/store/cron.ts ⟶ live 2026-10-17, `tests/live_backend.rs` `capabilities` section vs private backend (pointer clicks + independent REST read-back): two-click Delete → backend read-back gone
- [x] Pause/resume cron job — REST /api/cron/jobs/{id}/pause|resume — src/app/cron/cron-actions.ts ⟶ live 2026-10-17, `tests/live_backend.rs` `capabilities` section vs private backend (pointer clicks + independent REST read-back): switch → state paused, then scheduled
- [~] Trigger cron job now — REST /api/cron/jobs/{id}/trigger — src/app/cron/cron-actions.ts ⟶ Run now implemented behind a confirm click (30 min timeout, off the chat loop); live test checks only that one click runs nothing (last_run_at null) — a real run spends model calls
- [ ] Cron runs history — REST /api/cron/jobs/{id}/runs — src/app/cron/job-state.ts
- [~] Cron delivery targets — REST /api/cron/delivery-targets — src/api/cron.ts ⟶ backend client ready in `api::extras` (live-verified calls listed in docs/api/extras.md / extras-admin.md); UI not wired
- [~] Cron blueprints — REST /api/cron/blueprints — src/app/cron/blueprints.tsx ⟶ backend client ready in `api::extras` (live-verified calls listed in docs/api/extras.md / extras-admin.md); UI not wired
- [~] Instantiate blueprint — REST /api/cron/blueprints/instantiate — src/app/cron/blueprints.tsx ⟶ backend client ready in `api::extras` (live-verified calls listed in docs/api/extras.md / extras-admin.md); UI not wired
- [~] Cron jobs section in sidebar — local — src/app/chat/sidebar/cron-jobs-section.tsx ⟶ sidebar Scheduled opens the Scheduled tab (verified live); jobs are not listed inline in the sidebar
- [ ] Open cron run in chat — local — src/app/cron/open-cron-run.ts
- [ ] Cron changed event — WS cron.changed — src/store/cron.ts

## Artifacts / preview

- [ ] Artifacts library (list/grid) — local — src/app/artifacts/index.tsx
- [ ] Artifact detection from messages — local — src/lib/artifact-detect.ts
- [ ] Artifact store — local — src/store/artifacts.ts
- [ ] Preview pane (in-app browser) — local — src/app/chat/right-rail/preview.tsx
- [ ] Preview guest webview (Chromium) — local (Electron webview) (platform) — src/app/chat/right-rail/preview-guest-offscreen.ts
- [ ] Preview navigation bar + address — local — src/app/chat/right-rail/preview-browser-bar.tsx
- [ ] Preview console (JS console output) — local — src/app/chat/right-rail/preview-console.tsx
- [ ] Preview annotate (draw on page) — SRV preview.act — src/app/chat/right-rail/preview-annotate-card.tsx
- [ ] Preview act (click/type/scroll proxy) — SRV preview.act — src/lib/preview-act/*
- [ ] Preview read (text extraction) — SRV preview.read — src/app/chat/right-rail/preview-reader.ts
- [ ] Preview restart — RPC preview.restart — src/app/session/hooks/use-preview-routing.ts
- [ ] Preview file viewer — local — src/app/chat/right-rail/preview-file.tsx
- [ ] Preview script runner — local — src/app/chat/right-rail/preview-script-runner.ts
- [ ] Preview popout window — local (Electron) (platform) — src/app/chat/browser-popout-shell.tsx, right-rail/preview-popout-bridge.ts
- [ ] Preview open in external browser — local (Electron shell.openExternal) (platform) — src/store/preview-open-browser.ts
- [ ] Preview targets/tabs — local — src/store/preview.ts, preview-tabs-scope.ts
- [ ] Preview file-change watch — local (Electron preview-file-watch) — electron/preview-file-watch.ts
- [ ] Preview reachability (can a URL open?) — local — src/lib/preview-reach.ts
- [ ] Preview screenshot capture — local (Electron) — electron/preview-capture.ts

## Right panel (files / review / terminal)

- [ ] Right sidebar container + toggles — local — src/app/right-sidebar/index.tsx, src/store/right-side-toggle.ts
- [ ] File browser tree — local (Electron fs-ipc) — src/app/right-sidebar/files/tree.tsx
- [ ] File tree preferences — local — src/app/right-sidebar/files/prefs.ts
- [ ] File actions (open/reveal/copy path) — local (Electron) — src/app/right-sidebar/file-actions.tsx
- [ ] File drag-and-drop manager — local — src/app/right-sidebar/files/dnd-manager.ts
- [ ] Remote file picker — local + remote gateway — src/app/right-sidebar/files/remote-picker.tsx
- [~] Project tree source (files pane) — RPC projects.tree — src/app/right-sidebar/files/use-project-tree.ts ⟶ backend client ready in `api::workspace` (live-verified calls listed in docs/api/workspace.md); UI not wired
- [~] Review pane (source control) — REST /api/git/* — src/app/right-sidebar/review/index.tsx ⟶ backend client ready in `api::workspace` (live-verified calls listed in docs/api/workspace.md); UI not wired
- [ ] Review file tree — local — src/app/right-sidebar/review/file-tree.tsx
- [ ] Review churn bar (additions/deletions) — local — src/app/right-sidebar/review/churn-bar.tsx
- [ ] Review scope row (staged/unstaged/PR) — local — src/app/right-sidebar/review/scope-row.tsx
- [ ] Review ship bar (commit/push actions) — local (Electron git-ipc) — src/app/right-sidebar/review/ship-bar.tsx
- [ ] Review summary (agent-produced) — WS review.summary — src/store/review.ts
- [ ] Pull-requests list — REST /api/profiles/sessions/pull-requests — src/store/pull-requests.ts
- [~] GitHub auth probe — REST /api/git/gh-auth — src/store/suggestion-providers/github.ts ⟶ backend client ready in `api::workspace` (live-verified calls listed in docs/api/workspace.md); UI not wired
- [ ] Terminal pane (persistent PTY) — local (Electron terminal-ipc) — src/app/right-sidebar/terminal/instance.tsx
- [ ] Terminal tabs rail — local — src/app/right-sidebar/terminal/rail.tsx
- [ ] New terminal — local — src/app/right-sidebar/terminal/terminals.ts
- [ ] Terminal copy/paste (⌘C/⌘V or ⌃⇧C/⌃⇧V) — local — src/app/right-sidebar/terminal/clipboard.ts
- [ ] Terminal links (open URLs from output) — local — src/app/right-sidebar/terminal/links.ts
- [~] Agent terminal stream (agent runs a command in the pane) — RPC terminal.resize + WS agent.terminal.output — src/app/right-sidebar/terminal/agent-terminal-stream.ts ⟶ backend client ready in `api::workspace` (live-verified calls listed in docs/api/workspace.md); UI not wired
- [ ] Terminal font settings — local — src/app/settings/terminal-font-setting.tsx
- [ ] Terminal backend warning — local — src/store/terminal-backend-warning.ts
- [ ] Terminal toggle (⌃`) / new (⌃⇧`) / next/prev / close hotkeys — local — src/lib/keybinds/actions.ts
- [ ] Review toggle (⌘G) / files hotkey — local — src/lib/keybinds/actions.ts
- [ ] Terminal read (agent reads visible buffer) — SRV terminal.read — src/app/session/hooks/*

## Voice

- [~] Voice conversation (chained STT→agent→TTS) — RPC voice.toggle + REST /api/audio/* — src/app/chat/composer/hooks/use-voice-conversation.ts ⟶ backend client ready in `api::voice` (live-verified calls listed in docs/api/voice.md); UI not wired
- [~] GPT-Live voice mode — config voice.voice_chat_mode=gpt-live + REST /api/audio — src/app/chat/composer/hooks/use-voice-live-conversation.ts ⟶ backend client ready in `api::voice` (live-verified calls listed in docs/api/voice.md); UI not wired
- [ ] Push-to-talk / mic recorder — local MediaRecorder — src/app/chat/composer/hooks/use-mic-recorder.ts
- [~] Voice recorder (record → transcribe) — REST /api/audio/transcribe — src/app/chat/composer/hooks/use-voice-recorder.ts ⟶ backend client ready in `api::voice` (live-verified calls listed in docs/api/voice.md); UI not wired
- [ ] Dictation hotkey (unbound) — local — src/lib/keybinds/actions.ts (composer.dictate)
- [ ] Speech-to-text enable — config stt.enabled — src/app/settings/constants.ts
- [ ] STT provider select — config stt.provider — src/app/settings/constants.ts
- [ ] STT streaming (live transcript) — config stt.streaming + WS voice.partial — src/app/settings/constants.ts
- [ ] STT echo transcripts — config stt.echo_transcripts — src/app/settings/constants.ts
- [ ] Local STT model/language — config stt.local.* — src/app/settings/constants.ts
- [ ] Provider STT models (OpenAI/Groq/Mistral/xAI/DeepInfra/ElevenLabs) — config stt.* — src/app/settings/constants.ts
- [~] STT lease (warm/cool local engine) — REST /api/audio/stt-lease — src/lib/stt-lease.ts ⟶ backend client ready in `api::voice` (live-verified calls listed in docs/api/voice.md); UI not wired
- [ ] Text-to-speech provider — config tts.provider — src/app/settings/constants.ts
- [ ] TTS voices per provider (Edge/OpenAI/ElevenLabs/xAI/MiniMax/Mistral/Gemini/NeuTTS/KittenTTS/Piper/DeepInfra) — config tts.* — src/app/settings/constants.ts
- [ ] Read responses aloud — config voice.auto_tts — src/app/settings/constants.ts
- [~] Voice playback + queue — local + REST /api/audio/speak — src/lib/voice-playback.ts ⟶ backend client ready in `api::voice` (live-verified calls listed in docs/api/voice.md); UI not wired
- [ ] Voice stream (live PCM sibling WS) — local WS /api/voice — src/lib/voice-stream.ts
- [ ] Voice barge-in / interrupted — WS voice.interrupted — src/lib/voice-barge-in.ts
- [ ] Wake word (always-listening) — RPC wake.start|stop|status|feed|pause|resume — src/store/wake-word.ts
- [ ] Wake indicator window — local (Electron wake-indicator) — src/app/wake-indicator/wake-indicator-app.tsx
- [~] TTS lease (warm engine) — REST /api/audio/tts-lease — src/lib/tts-lease.ts ⟶ backend client ready in `api::voice` (live-verified calls listed in docs/api/voice.md); UI not wired
- [ ] Voice provider fields visibility — local — src/app/settings/voice-provider-fields.tsx, voice-field-visible.ts
- [ ] Max recording length — config voice.max_recording_seconds — src/app/settings/constants.ts
- [ ] Client-direct voice — config voice.client_direct — src/app/settings/constants.ts
- [ ] Speech text sanitization — local — src/lib/speech-text.ts
- [ ] Spoken reply formatting — local — src/lib/spoken-reply.ts

## Command palette & shortcuts

- [ ] Open command palette (⌘K / ⌘P) — local — src/app/command-palette/index.tsx
- [ ] Command palette search (settings/tools/sessions/actions) — local — src/app/settings/settings-search.ts
- [ ] Open command center (⌘.) — local — src/app/command-center/index.tsx
- [ ] Open settings (⌘,) — local — src/app/shell/hooks/use-overlay-routing.ts
- [ ] Open shortcuts panel (⌘/) — local — src/app/settings/keybind-settings.tsx
- [ ] Send message (Enter) — RPC prompt.submit — src/lib/keybinds/actions.ts (composer.send)
- [~] Steer (Enter) / queue (⌘Enter) — RPC session.steer / prompt.submit — src/lib/keybinds/actions.ts ⟶ backend client ready in `api::sessions` (live-verified calls listed in docs/api/sessions.md); UI not wired
- [ ] Send queued (⌘⇧K) — local — src/lib/keybinds/actions.ts
- [ ] Mention (@) / slash (/) / help (?) — local — src/lib/keybinds/actions.ts
- [ ] Input history (↑/↓) — local — src/lib/keybinds/actions.ts
- [ ] Cancel composer (Esc) — local — src/lib/keybinds/actions.ts
- [ ] Focus composer (⌘/Ctrl+L) — local — src/lib/keybinds/actions.ts
- [ ] Selection to composer (⌘/Ctrl+L) — local — src/lib/keybinds/actions.ts
- [ ] Model picker (⌘⇧M) — local — src/lib/keybinds/actions.ts
- [ ] Toggle voice (⌃B mac / ⌘⌥V else) — local — src/lib/keybinds/actions.ts
- [ ] Dictation (unbound) — local — src/lib/keybinds/actions.ts
- [ ] Reasoning up/down (unbound) — local — src/lib/keybinds/actions.ts
- [x] New session (⌘N) — RPC session.create — src/lib/keybinds/actions.ts ⟶ Ctrl+N, see Sessions & sidebar
- [ ] New tab (⌘T) — local — src/lib/keybinds/actions.ts
- [ ] New window (⌘⇧N) — local — src/lib/keybinds/actions.ts
- [ ] Next/prev session (⌃Tab / ⌃⇧Tab, ⌃PgDn/PgUp) — local — src/lib/keybinds/actions.ts
- [ ] Session slots (⌃1…⌃9) — local — src/lib/keybinds/actions.ts
- [~] Search sessions (⌘⇧F) — REST /api/sessions/search — src/lib/keybinds/actions.ts ⟶ backend client ready in `api::sessions` (live-verified calls listed in docs/api/sessions.md); UI not wired
- [ ] Toggle session pin (unbound) — local — src/lib/keybinds/actions.ts
- [~] Archive session (unbound) — RPC session.archive — src/lib/keybinds/actions.ts ⟶ backend client ready in `api::sessions` (live-verified calls listed in docs/api/sessions.md); UI not wired
- [ ] Scroll transcript page up/down (PgUp/PgDn) — local — src/lib/keybinds/actions.ts
- [ ] New worktree (⌘⇧B) — local — src/lib/keybinds/actions.ts
- [ ] Open folder as project (⌘O) — local — src/lib/keybinds/actions.ts
- [ ] Open profiles / capabilities / messaging / artifacts / cron / agents (unbound) — local — src/lib/keybinds/actions.ts
- [ ] Toggle sidebar (⌘B) — local — src/lib/keybinds/actions.ts
- [ ] Cycle sidebar grouping (unbound) — local — src/lib/keybinds/actions.ts
- [ ] Toggle right sidebar (⌘J) — local — src/lib/keybinds/actions.ts
- [ ] Toggle statusbar (⌘⇧S) — local — src/lib/keybinds/actions.ts
- [ ] Toggle tab strip (⌘⌥T) — local — src/lib/keybinds/actions.ts
- [ ] Toggle profile rail (unbound) — local — src/lib/keybinds/actions.ts
- [ ] Toggle simple mode (unbound) — local — src/lib/keybinds/actions.ts
- [ ] Toggle review pane (⌘G) — local — src/lib/keybinds/actions.ts
- [ ] Show files pane (unbound) — local — src/lib/keybinds/actions.ts
- [ ] Show browser preview (⌘⇧L) — local — src/lib/keybinds/actions.ts
- [ ] Toggle HUD (⌘⇧H) — local — src/lib/keybinds/actions.ts
- [ ] Show terminal (⌃`) — local — src/lib/keybinds/actions.ts
- [ ] New terminal (⌃⇧`) — local — src/lib/keybinds/actions.ts
- [ ] Next/prev terminal (⌃⇧↓ / ⌃⇧↑) — local — src/lib/keybinds/actions.ts
- [ ] Close terminal (⌃⇧W) — local — src/lib/keybinds/actions.ts
- [ ] Flip panes (⌘\) — local — src/lib/keybinds/actions.ts
- [ ] Close tab (⌘W) — local — src/lib/keybinds/actions.ts
- [ ] Reopen tab (⌘⇧T) — local — src/lib/keybinds/actions.ts
- [ ] Find in page (⌘F) — local — src/lib/keybinds/actions.ts
- [ ] Find next/prev (unbound) — local — src/lib/keybinds/actions.ts
- [ ] Toggle appearance mode (⇧X) — local — src/lib/keybinds/actions.ts (appearance.toggleMode)
- [ ] Tab slot jumps (⌘1…⌘9, passthrough) — local — src/lib/keybinds/actions.ts
- [ ] Name profile switch (⌘1…⌘9 / ⌘⌥1…⌘⌥9) — local — src/lib/keybinds/actions.ts
- [ ] Default profile switch (⌘D) — local — src/lib/keybinds/actions.ts
- [ ] Profile next/prev (⌘⇧] / ⌘⇧[) — local — src/lib/keybinds/actions.ts
- [ ] Toggle all profiles (⌘⇧0) — local — src/lib/keybinds/actions.ts
- [ ] Create profile (unbound) — local — src/lib/keybinds/actions.ts
- [ ] HUD snap to pointer (⌘⇧G, HUD-only) — local — src/lib/keybinds/actions.ts (KEYBIND_READONLY)
- [ ] Terminal copy/paste (platform chords) — local — src/lib/keybinds/actions.ts (KEYBIND_READONLY)
- [ ] Rebind / clear / reset keys + conflict detection — local — src/store/keybinds.ts
- [ ] Per-plugin contributed keybinds — local — src/lib/keybinds/plugin-actions.ts
- [ ] Keybind deep-link capture mode — local — src/store/keybinds.ts

## Settings (shell)

- [x] Settings overlay shell + search — local — src/app/settings/index.tsx, use-settings-search.ts
  ⟶ registry-driven panel: 19-group nav (active group's pages expanded, section dividers), Ctrl+, a search field over every page/control, "Applies to" profile pills — tests/live_backend.rs `settings` (live, private backend, 2026-10-17); shots in docs/screenshots/rs-settings-*.png
- [~] Settings deep-link (tab/page/field) — local — src/app/settings/use-setting-deep-link.ts
  ⟶ a search hit jumps to its page and highlights the row (live-tested); the Electron URL/tab deep-link has no native analogue — `HERMES_DESKTOP_OPEN_SETTINGS=<group>/<page>` opens a page for scripted screenshots
- [x] Settings manifest (searchable row registry) — local — src/app/settings/settings-manifest.ts
  ⟶ crates/hermes-core/src/settings.rs: every page and control in one table (labels, descriptions and option lists from the original; backend-config rows derived from /api/config/schema), ranked search, per-profile vs device store, conditional rows — 8 unit tests
- [-] Legacy tab redirects (connections→gateway, mcp→capabilities) — local — src/app/settings/moved-tabs.ts
  ⟶ unsupported: the native app has no URL-based settings tabs; each page carries one registry id, so there is nothing to redirect

## Settings — Model page

- [ ] Default model — config model / REST /api/model/set — src/app/settings/constants.ts
- [ ] Main model context override — config model_context_length — src/app/settings/constants.ts
- [ ] Reasoning effort — config agent.reasoning_effort — src/app/settings/config-subpages.ts
- [ ] Service tier — config agent.service_tier — src/app/settings/constants.ts
- [ ] Fallback models — config fallback_providers — src/app/settings/fallback-models-field.tsx
- [ ] Auxiliary (task) models — REST /api/model/auxiliary — src/app/settings/aux-task-rows.tsx
- [ ] MoA models — REST /api/model/moa — src/app/settings/model-settings.tsx

## Settings — Chat page

- [ ] Personality — config display.personality — src/app/settings/constants.ts
- [ ] Timezone — config timezone — src/app/settings/constants.ts
- [ ] Reasoning blocks — config display.show_reasoning — src/app/settings/constants.ts
- [ ] Image attachments mode — config agent.image_input_mode — src/app/settings/constants.ts
- [ ] Attachment size limit — local chat.attachmentSize — src/app/settings/settings-manifest.ts

## Settings — Appearance page

> Every `[~]` row renders in the panel and persists its value (device prefs in `prefs.json`, or the scoped profile's config through `PUT /api/config?profile=`) — the note names the part of the feature that is still missing.

- [~] Language / locale — local — src/app/settings/settings-manifest.ts (appearance.language)
  ⟶ writes display.language per profile (live-tested); no localisation catalog yet
- [~] Intro splash — local — src/app/settings/settings-manifest.ts (appearance.introSplash)
  ⟶ row renders and persists; the empty-chat greeting ignores it
- [~] Model pricing in picker — local — src/app/settings/settings-manifest.ts (appearance.modelPricing)
  ⟶ row renders and persists; the model picker shows no prices
- [~] Resume last session on launch — local — src/app/settings/settings-manifest.ts (appearance.resumeLastSession)
  ⟶ row renders and persists; launch always starts a new chat
- [~] Tips — local — src/app/settings/settings-manifest.ts (appearance.tips)
  ⟶ row renders and persists; no tips system
- [~] Tours — local — src/app/settings/settings-manifest.ts (appearance.tours)
  ⟶ row renders and persists; no tours
- [~] Theme / skin — local — src/app/settings/settings-manifest.ts (appearance.theme)
  ⟶ row renders and persists; the palette grid and palette application are not built (the row shows a labelled placeholder)
- [x] Dark/light/system mode — local — src/app/settings/constants.ts (MODE_OPTIONS)
  ⟶ applied live to the app's Theme global (mode switch is immediate); shot in light and dark (docs/screenshots/rs-settings-*.png)
- [~] UI scale — local — src/app/settings/settings-manifest.ts (appearance.uiScale)
  ⟶ row renders and persists; the layout does not scale yet
- [~] Chat text scale — local — src/app/settings/settings-manifest.ts (appearance.chatTextScale)
  ⟶ row renders and persists; conversation text does not scale yet
- [~] Chat font — local — src/app/settings/settings-manifest.ts (appearance.chatFont)
  ⟶ a plain text input writes the config key; the installed-font picker is not built
- [~] Terminal font — local — src/app/settings/settings-manifest.ts (appearance.terminalFont)
  ⟶ a plain text input writes the config key; there is no terminal panel yet
- [~] Interface mode (Simple/Advanced) — local — src/store/interface-mode.ts
  ⟶ row renders and persists; the shell always shows the advanced layout
- [~] Session list density — local — src/store/session-list-density.ts
  ⟶ row renders and persists; the sidebar keeps one density
- [~] Tab strip mode — local — src/store/tabstrip-prefs.ts
  ⟶ row renders and persists; no zone tab strip yet
- [~] Titlebar app actions side — local — src/app/settings/settings-manifest.ts (appearance.appActions)
  ⟶ row renders and persists; the titlebar layout is fixed
- [~] Minimize to tray — local (Electron) — src/app/settings/minimize-to-tray-setting.tsx
  ⟶ row renders and persists; no tray integration yet
- [~] Window translucency — local (Electron) — src/app/settings/settings-manifest.ts (appearance.translucency)
  ⟶ row renders and persists; the window is opaque
- [~] Backdrop / glass tint/fade/frost/area — local — src/store/backdrop.ts, src/app/settings/appearance-settings.tsx
  ⟶ row renders and persists; no backdrop rendering
- [~] File browser pane startup — local — src/store/layout.ts
  ⟶ row renders and persists; no file browser pane
- [~] Composer popout — local — src/store/composer-popout.ts
  ⟶ row renders and persists; the composer stays docked
- [~] User bubble opacity — local — src/store/user-bubble-transparency.ts
  ⟶ the slider writes the pref; user bubbles ignore it
- [~] Text direction (LTR/RTL) — local — src/store/text-direction.ts
  ⟶ row renders and persists; the layout is LTR only
- [~] Hide thread timeline — local — src/store/thread-timeline.ts
  ⟶ row renders and persists; no thread timeline
- [ ] Reactions (emoji) — local + config.set display.reactions — src/store/reactions-enabled.ts
- [~] Vibe hearts — local — src/store/vibe-hearts-enabled.ts
  ⟶ row renders and persists; no hearts
- [~] Tool view (product/technical) — local — src/store/tool-view.ts
  ⟶ row renders and persists; tool rows have one rendering
- [~] Hide code diffs — local — src/app/settings/settings-manifest.ts (appearance.hideCodeDiffs)
  ⟶ row renders and persists; diffs are always shown
- [~] Reasoning collapsed — local — src/store/reasoning-disclosure.ts
  ⟶ row renders and persists; reasoning blocks are not rendered yet
- [~] Embeds mode (ask/always/off) — local — src/store/embed-consent.ts
  ⟶ row renders and persists; no embed handling
- [~] Pet appearance — local — src/app/settings/pet-settings.tsx
  ⟶ row renders and persists; no pet
- [~] Video playback speed — local — src/store/video-playback-speed.ts
  ⟶ row renders and persists; no in-app video
- [~] Appearance plugin seam (appearance.extra) — local — src/app/settings/appearance-contrib.tsx
  ⟶ row renders and persists; plugins have no appearance contributions

## Settings — Workspace page

- [ ] Working directory — config terminal.cwd — src/app/settings/constants.ts
- [ ] Automatic repository discovery — config desktop.repo_scan_enabled — src/app/settings/constants.ts
- [ ] Repository discovery roots — config desktop.repo_scan_roots — src/app/settings/constants.ts
- [ ] Excluded repository paths — config desktop.repo_scan_exclude_paths — src/app/settings/constants.ts
- [ ] Code execution mode — config code_execution.mode — src/app/settings/constants.ts
- [ ] Persistent shell (SSH) — config terminal.persistent_shell — src/app/settings/constants.ts
- [ ] Environment passthrough — config terminal.env_passthrough — src/app/settings/constants.ts
- [ ] File read limit — config file_read_max_chars — src/app/settings/constants.ts
- [ ] Data-URL read limit — local — src/store/data-url-read-max.ts

## Settings — Safety page

- [ ] Approval mode — config approvals.mode — src/app/settings/constants.ts
- [ ] Approval timeout — config approvals.timeout — src/app/settings/constants.ts
- [ ] Confirm MCP reloads — config approvals.mcp_reload_confirm — src/app/settings/constants.ts
- [ ] Command allowlist — config command_allowlist — src/app/settings/constants.ts
- [ ] Redact secrets — config security.redact_secrets — src/app/settings/constants.ts
- [ ] Allow private URLs — config security.allow_private_urls — src/app/settings/constants.ts
- [ ] File checkpoints — config checkpoints.enabled — src/app/settings/constants.ts
- [ ] Shared metrics / telemetry consent — local — src/app/settings/shared-metrics-settings.tsx

## Settings — Browser page

- [ ] Use real browser profile — config browser.use_real_profile — src/app/settings/browser-real-profile-panel.tsx
- [ ] Browser private URLs — config browser.allow_private_urls — src/app/settings/constants.ts
- [ ] Local browser for private URLs — config browser.auto_local_for_private_urls — src/app/settings/constants.ts

## Settings — Memory & Context page

- [ ] Persistent memory — config memory.memory_enabled — src/app/settings/constants.ts
- [ ] User profile — config memory.user_profile_enabled — src/app/settings/constants.ts
- [ ] Memory budget — config memory.memory_char_limit — src/app/settings/constants.ts
- [ ] Profile budget — config memory.user_char_limit — src/app/settings/constants.ts
- [ ] Memory provider — config memory.provider — src/app/settings/constants.ts
- [ ] Context engine — config context.engine — src/app/settings/constants.ts
- [ ] Auto-compression — config compression.enabled — src/app/settings/constants.ts
- [ ] Compression threshold — config compression.threshold — src/app/settings/constants.ts
- [ ] Codex compression auto-raise — config compression.codex_gpt55_autoraise — src/app/settings/constants.ts
- [ ] Compression target ratio — config compression.target_ratio — src/app/settings/constants.ts
- [ ] Protected recent messages — config compression.protect_last_n — src/app/settings/constants.ts
- [ ] Compression aux timeout — config auxiliary.compression.timeout — src/app/settings/constants.ts

## Settings — Voice page

- [ ] Voice chat mode — config voice.voice_chat_mode — src/app/settings/constants.ts
- [ ] GPT-Live voice — config voice.gpt_live.voice — src/app/settings/constants.ts
- [ ] GPT-Live persona — config voice.gpt_live.instructions — src/app/settings/constants.ts
- [ ] TTS provider — config tts.provider — src/app/settings/constants.ts
- [ ] STT enabled — config stt.enabled — src/app/settings/constants.ts
- [ ] Echo transcripts — config stt.echo_transcripts — src/app/settings/constants.ts
- [ ] STT provider — config stt.provider — src/app/settings/constants.ts
- [ ] Live transcription — config stt.streaming — src/app/settings/constants.ts
- [ ] Read responses aloud — config voice.auto_tts — src/app/settings/constants.ts
- [ ] Edge voice — config tts.edge.voice — src/app/settings/constants.ts
- [ ] OpenAI TTS model/voice — config tts.openai.* — src/app/settings/constants.ts
- [ ] ElevenLabs voice/model — config tts.elevenlabs.* — src/app/settings/constants.ts
- [ ] xAI TTS voice/language/speed/tags/latency/sample/bitrate — config tts.xai.* — src/app/settings/constants.ts
- [ ] MiniMax/Mistral/Gemini/NeuTTS/KittenTTS/Piper/DeepInfra TTS — config tts.* — src/app/settings/constants.ts
- [ ] Local STT model/language — config stt.local.* — src/app/settings/constants.ts
- [ ] Provider STT models — config stt.{openai,groq,mistral,xai,deepinfra,elevenlabs}.* — src/app/settings/constants.ts
- [ ] Max recording length — config voice.max_recording_seconds — src/app/settings/constants.ts
- [ ] Client-direct voice — config voice.client_direct — src/app/settings/constants.ts

## Settings — Advanced page

- [ ] Enabled toolsets — config toolsets — src/app/settings/constants.ts
- [ ] Execution backend — config terminal.backend — src/app/settings/terminal-backend-panel.tsx
- [ ] Command timeout — config terminal.timeout — src/app/settings/constants.ts
- [ ] Container images (docker/singularity/modal/daytona) — config terminal.*_image — src/app/settings/constants.ts
- [ ] Terminal output limit / file page limit / line length — config tool_output.* — src/app/settings/constants.ts
- [ ] Checkpoint limit — config checkpoints.max_snapshots — src/app/settings/constants.ts
- [ ] Max agent steps — config agent.max_turns — src/app/settings/constants.ts
- [ ] API retries — config agent.api_max_retries — src/app/settings/constants.ts
- [ ] Service tier — config agent.service_tier — src/app/settings/constants.ts
- [ ] Tool-use enforcement — config agent.tool_use_enforcement — src/app/settings/constants.ts
- [ ] Subagent model/provider/turn limit/concurrency/timeout/reasoning — config delegation.* — src/app/settings/constants.ts
- [ ] In-app update local changes (stash/discard) — config updates.non_interactive_local_changes — src/app/settings/constants.ts
- [ ] Keep awake (off/while-working/always) — local (Electron power) — src/store/keep-awake.ts
- [ ] Disable F12 devtools — local (Electron) — src/store/disable-f12.ts
- [ ] Always open links externally — local — src/store/external-links.ts
- [ ] Warm bot backends (pool) — local — src/app/settings/pool-limits-setting.tsx
- [ ] Backend idle timeout (pool) — local — src/app/settings/pool-limits-setting.tsx
- [ ] Quick entry enable — local (Electron quick-entry) — src/app/settings/quick-entry-settings.tsx
- [ ] Quick entry shortcut — local (Electron) — src/app/settings/quick-entry-settings.tsx
- [ ] Reset onboarding (developer) — local + RPC onboarding.reset_setup_profile — src/app/settings/developer-settings.tsx

## Settings — Gateway / Connections page

- [ ] Connection mode (local/remote/cloud) — local (Electron connection-config) — src/app/settings/gateway-settings.tsx
- [ ] Remote URL + token — local (Electron) — src/app/settings/gateway-settings.tsx
- [ ] Extra gateway headers — local (Electron connection.json) (platform) — src/app/settings/gateway-settings.tsx
- [ ] SSH connection host — local (Electron ssh-config) — src/app/settings/ssh-host-selection.ts
- [ ] Cloud connection (sign-in, org/agent picker) — local + REST — src/app/settings/gateway-settings.tsx
- [ ] Keychain encryption of secrets — local (Electron secret-storage-policy) — src/app/settings/settings-manifest.ts (gateway.keychainEncryption)
- [ ] Connections registry — local — src/app/settings/connections-registry.tsx
- [~] Diagnostics (share logs) — RPC diagnostics.share_nous + REST /api/ops/debug-share — src/app/settings/settings-manifest.ts (gateway.diagnostics) ⟶ backend client ready in `api::system` (live-verified calls listed in docs/api/system.md); UI not wired
- [~] Gateway restart — REST /api/gateway/restart — src/lib/shared-gateway-restart.ts ⟶ backend client ready in `api::system` (live-verified calls listed in docs/api/system.md); UI not wired
- [~] Managed updates — local + REST /api/hermes/update — src/app/settings/managed-updates-section.tsx ⟶ backend client ready in `api::system` (live-verified calls listed in docs/api/system.md); UI not wired
- [~] Doctor / security audit / backup — REST /api/ops/doctor|security-audit|backup — src/api/system.ts ⟶ backend client ready in `api::system` (live-verified calls listed in docs/api/system.md); UI not wired
- [ ] Open logs — local (Electron) — src/app/settings/gateway-settings.tsx
- [ ] Environment override (HERMES_*) — local — src/app/settings/env-credentials.tsx

## Settings — Keybinds page

- [ ] Shortcuts list + capture/rebind — local — src/app/settings/keybind-settings.tsx
- [ ] HUD modifier gesture — local (Electron hud-modifier) — src/app/settings/hud-modifier-settings.tsx
- [ ] Screenshot shortcut — local (Electron screenshot) — src/app/settings/screenshot-settings.tsx

## Settings — Notifications page

- [ ] Enable all notifications — local — src/app/settings/notifications-settings.tsx
- [ ] Approval notifications — local — src/store/native-notifications.ts
- [ ] Input notifications — local — src/store/native-notifications.ts
- [ ] Turn-done notifications — local — src/store/native-notifications.ts
- [ ] Turn-error notifications — local — src/store/native-notifications.ts
- [ ] Background-done notifications — local — src/store/native-notifications.ts
- [ ] Credits notifications — local — src/store/native-notifications.ts
- [ ] Plugin notifications — local — src/store/native-notifications.ts
- [ ] Completion sound — local — src/store/completion-sound.ts

## Settings — Sessions page

- [ ] Auto-archive stale sessions — local — src/app/settings/sessions-settings.tsx
- [ ] Default directory — local — src/app/settings/sessions-settings.tsx
- [~] Archived sessions list — REST /api/profiles/sessions — src/app/settings/sessions-settings.tsx ⟶ backend client ready in `api::sessions` (live-verified calls listed in docs/api/sessions.md); UI not wired

## Settings — Vault page

- [~] Vault credentials list — RPC vault.list — src/app/settings/vault-settings.tsx ⟶ backend client ready in `api::extras` (live-verified calls listed in docs/api/extras.md / extras-admin.md); UI not wired
- [~] Add credential — RPC vault.add — src/app/settings/vault-settings.tsx ⟶ backend client ready in `api::extras` (live-verified calls listed in docs/api/extras.md / extras-admin.md); UI not wired
- [~] Remove credential — RPC vault.remove — src/app/settings/vault-settings.tsx ⟶ backend client ready in `api::extras` (live-verified calls listed in docs/api/extras.md / extras-admin.md); UI not wired
- [~] Lock/unlock vault — RPC vault.lock, vault.unlock — src/app/settings/vault-settings.tsx ⟶ backend client ready in `api::extras` (live-verified calls listed in docs/api/extras.md / extras-admin.md); UI not wired
- [~] Vault sources — RPC vault.sources, vault.source.set — src/app/settings/vault-settings.tsx ⟶ backend client ready in `api::extras` (live-verified calls listed in docs/api/extras.md / extras-admin.md); UI not wired

## Settings — Providers & Keys page

- [ ] Provider accounts (OAuth) — REST /api/providers/oauth/* — src/app/settings/providers-settings.tsx
- [~] API keys (env vars) — REST /api/env, /api/env/reveal — src/app/settings/keys-settings.tsx ⟶ backend client ready in `api::system` (live-verified calls listed in docs/api/system.md); UI not wired
- [ ] Custom endpoints — REST /api/providers/custom-endpoints — src/app/settings/custom-endpoints-settings.tsx
- [ ] Local models — REST /api/local-models/* — src/app/settings/local-models-settings.tsx
- [ ] Tool keys (per-provider tool credentials) — local — src/app/settings/keys-settings.tsx (view 'tools')
- [ ] Provider groups (Nous/Fireworks/OpenRouter/Anthropic/xAI/Google/DeepSeek/…) — local — src/app/settings/constants.ts (PROVIDER_GROUPS)

## Settings — Plugins page

- [ ] Per-plugin settings pages — local — src/app/settings/plugin-settings.tsx
- [ ] Plugin settings form (declarative fields) — local — src/app/settings/plugin-settings-form.tsx
- [~] Plugin install from catalog — RPC plugins.catalog.lookup — src/app/settings/plugin-install-modal.tsx ⟶ backend client ready in `api::plugins` (live-verified calls listed in docs/api/plugins.md); UI not wired

## Settings — Billing page

- [ ] Billing overview (plan/credits) — RPC billing.state — src/app/settings/billing/index.tsx
- [ ] Plans view (upgrade/change) — RPC subscription.* — src/app/settings/billing/plans-view.tsx
- [ ] Auto-reload credits — RPC billing.auto_reload — src/app/settings/billing/auto-reload-row.tsx
- [ ] Charge / charge status — RPC billing.charge, billing.charge_status — src/app/settings/billing/use-charge-poller.ts
- [ ] Step-up verification — RPC billing.step_up — src/app/settings/billing/use-step-up.ts
- [ ] Subscription change/preview/resume/upgrade — RPC subscription.change|preview|resume|upgrade — src/app/settings/billing/use-subscription-change.ts

## Settings — About page

- [~] App updates (check/download) — REST /api/hermes/update + Electron updater — src/app/settings/about-settings.tsx ⟶ backend client ready in `api::system` (live-verified calls listed in docs/api/system.md); UI not wired
- [ ] Update channel — local — src/app/settings/update-channel-row.tsx, src/store/updates-channel.ts
- [ ] Version label + distribution — local — src/lib/version-label.ts, distribution-label.ts
- [ ] Uninstall Hermes — local (Electron desktop-uninstall) — src/app/settings/uninstall-section.tsx
- [ ] Credits / open-source licenses — local — src/app/settings/about-settings.tsx

## Appearance / themes

- [ ] Built-in themes (light/dark/system) — local — src/themes/*
- [ ] Theme marketplace (browse themes) — local — src/app/command-palette/marketplace-theme-page.tsx
- [ ] Skin changed event (gateway-driven skin) — WS skin.changed — src/store/notifications.ts
- [ ] Local skin override — local (Electron local-skin) — electron/local-skin.ts
- [ ] Translucency / glass effects — local (Electron translucency) — src/store/translucency.ts
- [ ] Backdrop blur — local — src/store/backdrop.ts
- [ ] Particles background — local — src/components/particles/*
- [ ] Code editor theme — local — src/components/chat/code-editor-theme.ts
- [ ] Brand icons / product identity — local — src/lib/brand-icon.ts, product-identity.cjs
- [ ] Profile color swatches — local — src/lib/profile-color.ts
- [ ] Starmap visualization (session map) — local — src/app/starmap/*
- [ ] Pet overlay appearance — local — src/app/pet-overlay/pet-overlay-app.tsx

## Notifications

- [ ] In-app toast notifications — local — src/store/notifications.ts
- [ ] Butterbar (top banner notices) — local — src/store/butterbar.ts, src/app/shell/butterbar.tsx
- [ ] Terms/consent butterbar — local — src/app/shell/terms-butterbar.tsx
- [ ] Native OS notifications — local (Electron notification-ipc) — src/store/native-notifications.ts
- [ ] Notification click actions — local (Electron notification-actions) — electron/notification-actions.ts
- [ ] Notification registry (native) — local (Electron) — electron/notification-registry.ts
- [ ] Agent notices (in-chat notice rows) — WS notice — src/store/agent-notices.ts
- [ ] Notification clearing — WS notification.clear — src/store/notifications.ts
- [ ] Tip popups — WS tip.show — src/store/tips.ts, src/components/tips/*
- [ ] Completion sound — local — src/lib/completion-sound.ts
- [ ] Notifications settings page — local — src/app/settings/notifications-settings.tsx

## Updates

- [ ] Auto-updater (macOS electron-updater) — local (Electron) (platform) — electron/app-updater.ts, electron/updater/mac.ts
- [ ] Windows MSIX/AppInstaller update — local (Electron) (platform) — electron/appinstaller-checker.ts
- [ ] Source-checkout update handoff — local (Electron) (platform) — electron/updater/checkout.ts
- [ ] Update gate / marker — local (Electron) (platform) — electron/update-gate.ts, electron/update-marker.ts
- [~] Update check (backend) — REST /api/hermes/update/check — src/store/updates.ts ⟶ backend client ready in `api::system` (live-verified calls listed in docs/api/system.md); UI not wired
- [ ] Managed (gateway) updates — local + REST — src/store/managed-updates.ts
- [ ] Updates overlay UI — local — src/app/updates-overlay.tsx
- [ ] Update channel select — local — src/app/settings/update-channel-row.tsx
- [ ] Pre-update backup config — local (Electron) — electron/pre-update-backup-config.ts

## Window / tray / HUD / pet

- [ ] Main window lifecycle / state — local (Electron) — electron/main-window-lifecycle.ts, electron/window-state.ts
- [ ] Window controls (custom titlebar) — local (Electron) — src/app/shell/titlebar-controls.tsx, electron/window-controls.ts
- [ ] Titlebar app actions (left/right) — local — src/app/shell/titlebar.ts
- [ ] Statusbar (custom status items) — local — src/app/shell/statusbar-controls.tsx, hooks/use-statusbar-items.tsx
- [~] System resources statusbar (CPU/RAM/battery) — local (Electron system.battery) — src/app/shell/system-resources-statusbar.tsx ⟶ backend client ready in `api::system` (live-verified calls listed in docs/api/system.md); UI not wired
- [ ] Tray icon + menu — local (Electron) (platform) — electron/tray-host.ts
- [ ] Minimize to tray — local (Electron) (platform) — electron/minimize-to-tray.ts
- [ ] HUD mode shell (chrome-free floating chat) — local (Electron HUD) — src/app/hud/hud-shell.tsx
- [ ] HUD modifier gesture (tap modifier to summon) — local (Electron native) — electron/hud-modifier.ts, electron/hud-modifier-monitor-*
- [ ] HUD snap to pointer (⌘⇧G) — local (Electron) — electron/hud-snap.ts
- [ ] HUD click-through + glass — local — src/app/hud/click-through.ts, src/app/hud/glass.ts
- [ ] HUD game overlay — local (Electron) — src/app/hud/game-overlay.ts, electron/hud-game-overlay.ts
- [ ] Quick Entry mini composer (global shortcut) — local (Electron) (platform) — src/app/quick-entry/quick-entry-app.tsx
- [ ] Quick Entry default shortcut (⌘⇧Space) — local — src/store/quick-entry.ts
- [ ] Pet overlay (desktop companion) — RPC pet.info, pet.select + local — src/app/pet-overlay/pet-overlay-app.tsx
- [ ] Pet roam / roam behavior — local — src/components/pet/use-pet-roam.ts, roam-behavior.ts
- [ ] Pet hatch from egg — RPC pet.hatch — src/components/pet/pet-egg-hatch.tsx
- [ ] Pet gallery — RPC pet.gallery — src/app/pet-generate/pet-generate-content.tsx
- [ ] Pet generation (AI) — RPC pet.generate, pet.generate.status — src/store/pet-generate.ts
- [ ] Pet scale / rename / remove — RPC pet.scale|rename|remove — src/store/pet-generate.ts
- [ ] Pet info bubbles / reactions — RPC pet.info — src/components/pet/pet-bubble.tsx
- [ ] Pet zoom gesture / star shower — local — src/components/pet/use-pet-zoom-gesture.ts, pet-star-shower.tsx
- [ ] Wake indicator window (always-listening cue) — local (Electron) — src/app/wake-indicator/wake-indicator-app.tsx
- [ ] Solve-in-page challenge window (free-tier captcha) — local (Electron) — electron/challenge-window.ts
- [ ] Window zoom / UI scale (Electron zoom) — local (Electron) — electron/zoom.ts, src/store/zoom.ts
- [ ] Vibrancy / translucency window — local (Electron) (platform) — electron/translucency.ts
- [ ] Behind/below window (HUD behind app) — local (Electron) — electron/window-below.ts

## Onboarding / auth / billing

- [ ] First-run setup gate (local install vs connect) — local (Electron) — src/components/onboarding/*, electron/first-run-setup-gate.ts
- [ ] Bootstrap runner (provision bundled/source runtime) — local (Electron) (platform) — electron/bootstrap-runner.ts
- [ ] Onboarding state — RPC onboarding.state, onboarding.mark_seen — src/store/onboarding.ts
- [ ] Ensure setup profile/session — RPC onboarding.ensure_setup_profile|ensure_setup_session — src/app/contrib/onboarding-kickoff.ts
- [ ] Onboarding chat (setup assistant) — local + RPC — src/components/onboarding-chat/gate.tsx
- [ ] Provider OAuth sign-in — REST /api/providers/oauth/* — src/store/onboarding.ts
- [ ] Native OAuth window — local (Electron) (platform) — electron/native-oauth.ts, electron/native-oauth-login.ts
- [ ] Free-tier offer / provision — RPC free_tier.provision, free_tier.status — src/store/free-tier.ts
- [ ] Free-tier sign-in — RPC free_tier.claim_nudge — src/store/free-tier-sign-in.ts
- [ ] Free-tier challenge — WS free_tier.challenge + RPC free_tier.challenge_result — src/store/free-tier-challenge.ts
- [ ] Model confirmation step — REST /api/model/set, /api/providers/validate — src/store/onboarding.ts
- [~] Setup runtime check — RPC setup.runtime_check, setup.status — src/plugins/hermes-bots/create-dialog.tsx ⟶ backend client ready in `api::system` (live-verified calls listed in docs/api/system.md); UI not wired
- [ ] Billing gate (credit exhausted block) — local — src/store/billing-block.ts
- [ ] Billing state + plans — RPC billing.state, subscription.* — src/app/settings/billing/use-billing-state.ts
- [ ] Cloud team change — local — src/app/settings/cloud-team-change.ts
- [ ] Intro splash / first-run product tour — local — src/lib/tour/*, src/app/chat/built-in-tour.ts
- [ ] Onboarding kill/skip metrics — RPC shared_metrics.desktop_onboarding — src/app/contrib/hooks/desktop-onboarding-metrics.ts

## Remote connections

- [ ] Connect to existing Hermes gateway (URL) — local (Electron connection-config) — src/app/settings/gateway-settings.tsx
- [ ] Gateway probe (HTTP+WS reachability) — local (Electron gateway-ws-probe) — electron/gateway-ws-probe.ts
- [ ] Remote auth modes (token/OAuth) — local (Electron native-auth-decisions) — electron/native-auth-decisions.ts
- [ ] SSH remote lifecycle (spawn gateway over ssh) — local (Electron ssh-*) (platform) — electron/remote-lifecycle.ts, electron/ssh-connection.ts
- [ ] SSH control-master reuse — local (Electron) (platform) — electron/ssh-control-master-holders.ts
- [ ] SSH isolated keepalive — local (Electron) (platform) — electron/ssh-isolated-keepalive.ts
- [ ] SSH bootstrap (install hermes on host) — local (Electron) (platform) — electron/ssh-bootstrap-coordinator.ts
- [ ] Remote session cookies / portal — local (Electron) (platform) — electron/remote-session-cookies.ts, electron/portal-session.ts
- [ ] Remote OAuth ticket — local (Electron) (platform) — electron/remote-oauth-ticket.ts
- [ ] Remote liveness / reauth — local (Electron) (platform) — electron/remote-liveness.ts, electron/remote-reauth-latch.ts
- [ ] Hermes Cloud discovery — local + REST /api/agents — electron/cloud-discovery.ts, src/store/connections.ts
- [ ] Cloud agent provisioning — local + RPC — src/store/connections.ts
- [ ] Connections registry (local/remote/cloud list) — local — src/store/connections.ts
- [ ] Connection switcher in sidebar — local — src/app/chat/sidebar/connection-switcher.tsx
- [ ] Connection glyph / health pill — local — src/app/chat/sidebar/connection-glyph.tsx, src/lib/gateway-health-pill.ts
- [ ] Per-connection window state — local (Electron) — electron/connection-window-state.ts
- [ ] Remote file picker / path bridging — local (Electron wsl-path-bridge) — src/app/right-sidebar/files/remote-picker.tsx
- [ ] Managed SSH update — local (Electron) — electron/managed-ssh-update.ts
- [ ] Extra gateway headers (proxy auth) — local (Electron) (platform) — src/app/settings/gateway-settings.tsx
- [ ] WSL/WSLg support (clipboard, launch) — local (Electron) (platform) — electron/wslg-launch.ts, electron/wsl-clipboard-image.ts

## Extras: webhooks, imports, subagents, learning, telemetry

- [~] Webhooks list — REST /api/webhooks — src/app/webhooks/index.tsx ⟶ backend client ready in `api::messaging` (live-verified calls listed in docs/api/messaging.md); UI not wired
- [~] Enable webhook — REST /api/webhooks/enable — src/api/messaging.ts ⟶ backend client ready in `api::messaging` (live-verified calls listed in docs/api/messaging.md); UI not wired
- [~] Delete webhook — REST /api/webhooks/{name} (DELETE) — src/app/webhooks/index.tsx ⟶ backend client ready in `api::messaging` (live-verified calls listed in docs/api/messaging.md); UI not wired
- [ ] Webhook enabled toggle — REST /api/webhooks/{name}/enabled — src/api/messaging.ts
- [~] Session import (foreign agent sessions) — RPC session.foreign.list|preview|import — src/app/session-import/index.tsx, src/app/session-import/api.ts ⟶ backend client ready in `api::sessions` (live-verified calls listed in docs/api/sessions.md); UI not wired
- [~] Subagents list — RPC subagent.list — src/store/subagents.ts ⟶ backend client ready in `api::system` (live-verified calls listed in docs/api/system.md); UI not wired
- [~] Subagent tail — RPC subagent.tail — src/store/subagents.ts ⟶ backend client ready in `api::system` (live-verified calls listed in docs/api/system.md); UI not wired
- [~] Subagent steer/interrupt — RPC subagent.steer, subagent.interrupt — src/store/subagents.ts ⟶ backend client ready in `api::system` (live-verified calls listed in docs/api/system.md); UI not wired
- [~] Subagent spawn tree save/load/list — RPC spawn_tree.save|load|list — src/store/subagents.ts ⟶ backend client ready in `api::system` (live-verified calls listed in docs/api/system.md); UI not wired
- [~] Delegation status/pause — RPC delegation.status, delegation.pause — src/store/background-delegation.ts ⟶ backend client ready in `api::system` (live-verified calls listed in docs/api/system.md); UI not wired
- [~] Background prompt (BTW) — RPC prompt.background, prompt.btw; WS btw.complete — src/app/session/hooks/use-prompt-actions/slash.ts ⟶ backend client ready in `api::sessions` (live-verified calls listed in docs/api/sessions.md); UI not wired
- [~] Agents page (roster) — local + RPC session.active_list — src/app/agents/index.tsx ⟶ backend client ready in `api::sessions` (live-verified calls listed in docs/api/sessions.md); UI not wired
- [~] Learning graph — REST /api/learning/graph — src/api/skills.ts ⟶ backend client ready in `api::extras` (live-verified calls listed in docs/api/extras.md / extras-admin.md); UI not wired
- [~] Learning node detail/edit/delete — REST /api/learning/node + RPC learning.* — src/api/skills.ts ⟶ backend client ready in `api::extras` (live-verified calls listed in docs/api/extras.md / extras-admin.md); UI not wired
- [~] Learning frames — RPC learning.frames — src/app/learning/archive-skill-confirm-dialog.tsx ⟶ backend client ready in `api::extras` (live-verified calls listed in docs/api/extras.md / extras-admin.md); UI not wired
- [~] Insights — RPC insights.get — src/hermes.ts ⟶ backend client ready in `api::system` (live-verified calls listed in docs/api/system.md); UI not wired
- [ ] Starmap (session/learning map + timeline) — local — src/app/starmap/star-map.tsx, src/app/starmap/timeline.tsx
- [ ] Starmap share code + controls — local — src/app/starmap/share-code.ts, src/app/starmap/share-controls.tsx
- [~] Command center (command dispatch console) — RPC command.dispatch, command.resolve — src/app/command-center/index.tsx ⟶ backend client ready in `api::sessions` (live-verified calls listed in docs/api/sessions.md); UI not wired
- [~] Command center maintenance (doctor/audit/backup) — REST /api/ops/* — src/app/command-center/maintenance.tsx ⟶ backend client ready in `api::system` (live-verified calls listed in docs/api/system.md); UI not wired
- [ ] Desktop telemetry (feature use/daily) — RPC shared_metrics.* — src/store/desktop-metrics.ts
- [ ] Startup latency reporting — RPC shared_metrics.startup_latency — src/app/gateway/report-startup-latency.ts
- [ ] Friction/dislike reporting — RPC shared_metrics.desktop_friction|desktop_dislike — src/store/shared-metrics.ts
- [~] Diagnostics share (Nous) — RPC diagnostics.share_nous — src/store/send-diagnostics.ts ⟶ backend client ready in `api::system` (live-verified calls listed in docs/api/system.md); UI not wired
- [ ] i18n backend language packs — RPC i18n.languages, i18n.catalog — src/i18n/backend-packs.ts
- [ ] Slash command catalog (desktop) — local registry — src/lib/desktop-slash-commands.ts, src/lib/desktop-slash-registry.json
- [ ] Electron preload bridge (native API surface) — local (Electron) — electron/preload.ts
- [ ] CDP / dev inspector — local (Electron) (platform) — electron/dev-cdp.ts
- [ ] Renderer accessibility tree — local (Electron) (platform) — electron/renderer-accessibility.ts
- [ ] Crash forensics / lag watchdog — local (Electron) (platform) — electron/crash-forensics.ts, electron/main-process-lag-watchdog.ts
- [ ] Log rotation + desktop.log — local (Electron) (platform) — electron/log-rotation.ts

---

_Generated from source. Total items: 793. Sections: 45._
