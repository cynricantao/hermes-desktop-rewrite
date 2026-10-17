# API: sessions

Typed client for session listing, live-session control, transcript paging, branching, events,
completions, approvals/clarify answers and message reactions. Implemented in
`crates/hermes-core/src/api/sessions.rs`; live checks in
`crates/hermes-core/tests/live_api_sessions.rs`.

Two id spaces matter: `session.create`/`session.activate` answer a **runtime id** (e.g.
`f15a813f`) plus a **stored id** (e.g. `20261017_174917_874920`). Live RPCs (`session.close`,
`session.status`, `session.control`, `session.archive`, `message.react`, …) take the runtime id;
REST routes and `session.delete` take the stored id. `session_activate` also returns the stored
id as `session_key`.

## Feature → function

| Feature | Function | Backend call | Notes |
|---|---|---|---|
| List sessions | `sessions_list` / `sessions_list_request` | `GET /api/sessions` | `SessionListQuery`: profile, limit (≤100), offset, min_messages, archived (`exclude`/`only`/`include`), order (`created`/`recent`), source(s), exclude_sources, cwd_prefix, full. Defaults applied by the backend. |
| All-profile list | `profiles_sessions` / `profiles_sessions_request` | `GET /api/profiles/sessions` | Same query type; empty `profile` = every profile. Adds `profile_totals`, per-profile `errors`. |
| Search sessions | `sessions_search` / `sessions_search_request` | `GET /api/sessions/search` | Id + FTS content + title lanes, deduped by compression lineage. |
| Session detail | `session_detail` / `session_detail_request` | `GET /api/sessions/{id}` | Stored id. Carries `runtime_revision` / `runtime_generation` used by the REST mutation guards. |
| Messages page | `session_messages` / `session_messages_request` | `GET /api/sessions/{id}/messages` | `limit` (≤500), `offset`, `order` (`oldest`/`latest`), `include_compacted`, `include_inline_images` (default true ⇒ param omitted). |
| Messages around a prompt | `session_messages_around` / `session_messages_around_request` | `GET /api/sessions/{id}/messages/around` | Required `row_id` must be a user prompt row; bounded page (≤120). |
| Sidebar slices | `profiles_sessions_sidebar` / `profiles_sessions_sidebar_request` | `GET /api/profiles/sessions/sidebar` | Batched recents / cron / messaging slices. |
| Activate session | `session_activate` / `session_activate_call` | `session.activate` | Attach a stored session to the runtime; `omit_messages` skips the transcript. |
| Live session list | `session_active_list` / `session_active_list_call` | `session.active_list` | `current_session_id` marks the focused one. |
| Most recent session | `session_most_recent` / `session_most_recent_call` | `session.most_recent` | Stored id/title/source. |
| Status text | `session_status` / `session_status_call` | `session.status` | Human-readable block (empty when idle). |
| Control state | `session_control_read` / `session_control_read_call` | `session.control.read` | Goal / loop / heartbeat snapshot; empty snapshot (revision `""`) when no controls exist. |
| Apply control | `session_control` / `session_control_call` | `session.control` | `action` is a backend intent string (e.g. `heartbeat.clear`); optional `args`. |
| Set cwd | `session_cwd_set` / `session_cwd_set_call` | `session.cwd.set` | Live session only; the backend probes git in the new directory. |
| Move workspace | `session_workspace_move` / `session_workspace_move_call` | `session.workspace.move` | Same effect keyed by `session_key`. |
| Close session | `session_close` / `session_close_call` | `session.close` | Ends the live runtime session (stored rows untouched). **Destructive** to in-flight work. |
| Archive by id | `session_archive` / `session_archive_call` | `session.archive` | Toggle; returns the stored `session_key`. |
| Archive by key | `session_archive_key` / `session_archive_key_call` | `session.archive` | `session_key` form (hosted rooms). |
| Hide in sidebar | `session_set_hidden` / `session_set_hidden_call` | `session.set_hidden` | By live session id. |
| History | `session_history` / `session_history_call` | `session.history` | Full gateway-projected transcript. |
| Context breakdown | `session_context_breakdown` / `session_context_breakdown_call` | `session.context_breakdown` | Per-category tokens + context files. |
| Events since | `session_events_since` / `session_events_since_call` | `session.events.since` | Replay the event ring after `last_seen`; `truncated` ⇒ refetch. Event objects are free-form. |
| Event stats | `session_events_stats` / `session_events_stats_call` | `session.events.stats` | Ring counters. |
| Foreign sessions | `session_foreign_list` / `session_foreign_list_call` | `session.foreign.list` | Sessions importable from other hosts/stores. |
| Branch (stored) | `session_branch_stored` / `session_branch_stored_call` | `session.branch_stored` | Copies the stored transcript into a new session; no model call. Parent must be the **stored** id. |
| Branch last turns | `session_branch` / `session_branch_call` | `session.branch` | **Spends model calls.** |
| Branch whole | `session_branch_whole` / `session_branch_whole_call` | `session.branch_whole` | **Spends model calls.** |
| Save session | `session_save` / `session_save_call` | `session.save` | Writes the `session_<id>.json` snapshot. |
| Undo turn | `session_undo` / `session_undo_call` | `session.undo` | **Destructive** to transcript rows; optional backend `intent`. |
| Compress | `session_compress` / `session_compress_call` | `session.compress` | **Spends model calls.** |
| Steer | `session_steer` / `session_steer_call` | `session.steer` | **Spends model calls** (mid-turn guidance). |
| Redirect | `session_redirect` / `session_redirect_call` | `session.redirect` | **Spends model calls.** |
| Usage | `session_usage` / `session_usage_call` | `session.usage` | **Network** (provider limits / Nous credits). |
| Pending approvals | `approval_pending` / `approval_pending_call` | `approval.pending` | Read-only. |
| Approval seen | `approval_received` / `approval_received_call` | `approval.received` | Marks a prompt delivered. |
| Answer request | `request_answer` / `request_answer_call` | `request.answer` | Answers a clarify/question request; unknown id answers `{"status":"expired"}`. |
| Clarify lock | `clarify_lock` / `clarify_lock_call` | `clarify.lock` | Locks one question's answer; unknown id ⇒ `expired`. |
| Path completion | `complete_path` / `complete_path_call` | `complete.path` | `@path` completion. |
| One-shot LLM | `llm_oneshot` / `llm_oneshot_call` | `llm.oneshot {session_id?, template?, instructions?, input?, variables?, task?, max_tokens?, temperature?, profile}` | **Spends model calls**; `LlmOneshotOptions` carries `variables` / `task` (default `title_generation`) / `max_tokens` (1024) / `temperature` (0.3). Builder-only. |
| Image generation | `image_generate` / `image_generate_call` | `image.generate {prompt?, aspect_ratio?, probe?, max_bytes?}` | **Spends model calls / network**; `probe:true` answers availability only but is not run live (batch rule). `image_data` (data URL) omitted when the download failed. Builder-only. |
| Preview restart | `preview_restart` / `preview_restart_call` | `preview.restart {session_id, url, cwd?, context?}` | **Spawns a hidden side agent** that diagnoses the URL and deliberately leaves a restarted dev server running. Builder-only. |
| Slash completion | `complete_slash` / `complete_slash_call` | `complete.slash` | Commands + skills. |
| Detect drop | `input_detect_drop` / `input_detect_drop_call` | `input.detect_drop` | Dragged-file reference in composer text. |
| Collapse paste | `paste_collapse` / `paste_collapse_call` | `paste.collapse` | Placeholder + on-disk path + line count. |
| Command catalog | `commands_catalog` / `commands_catalog_call` | `commands.catalog` | All commands/skills for the composer. |
| Resolve command | `command_resolve` / `command_resolve_call` | `command.resolve` | Name → canonical/description/category. |
| Background prompt | `prompt_background` / `prompt_background_call` | `prompt.background` | **Spends model calls.** |
| Side question | `prompt_btw` / `prompt_btw_call` | `prompt.btw` | **Spends model calls.** |
| React / unreact | `message_react` / `message_react_call` | `message.react` | `emoji: None` clears that author's reaction; `row_id` from `Message.row_id`. |

## Live verification

`HERMES_DESKTOP_PORT=37273 cargo test -p hermes-core --test live_api_sessions -- --ignored --nocapture`
→ **69 checks, 0 failed**. Run on 2026-10-17 against the private backend; the throwaway
session (`session.create` with two seeded messages) is closed, deleted and confirmed 404.

Exercised live: `session.activate`, `session.active_list`, `session.most_recent`, `session.status`,
`session.control.read`, `session.control` (`heartbeat.clear`), `session.history`,
`session.context_breakdown`, `session.events.since`, `session.events.stats`, `session.foreign.list`,
`session.cwd.set`, `session.workspace.move`, `session.archive` (±), `session.set_hidden` (±),
`session.close`, `session.branch_stored` (plus history/close/delete of the branch),
`message.react` (set + clear), `request.answer` / `clarify.lock` (unknown ids → `expired`),
`complete.path`, `complete.slash`, `input.detect_drop`, `paste.collapse`, `commands.catalog`,
`command.resolve`, and the REST routes `GET /api/sessions`, `/search`, `/{id}`,
`/{id}/messages`, `/{id}/messages/around`, `/api/profiles/sessions`,
`/api/profiles/sessions/sidebar`.

## Not run live (and why)

- `session.save`, `session.undo`, `session.branch`, `session.branch_whole`, `approval.pending`,
  `approval.received`: the private backend has no AI provider (`5032 not connected to any AI
  provider`); builders and unit tests only.
- `session.compress`, `session.steer`, `session.redirect`, `session.usage`,
  `prompt.background`, `prompt.btw`: builder + unit tests only by the safety rules (model spend
  or network).
- `llm.oneshot`, `image.generate` (even `probe:true`, which is availability-only),
  `preview.restart`: model spend / provider call / a hidden side agent that leaves a background
  server running.
- REST mutations (`PATCH`/`DELETE /api/sessions/{id}`, bulk-delete, import, prune,
  owner-backfill) are out of scope. The live test's cleanup used the out-of-scope
  `session.delete` RPC with the throwaway stored id, as the brief allows.

## Not found / not implemented

- `session.create`, `session.resume`, `session.list`, `session.delete`, `session.interrupt`,
  `session.title`, `prompt.submit`, `approval.respond`, `slash.exec`, `command.dispatch` and all
  attachment methods (`image.*`, `file.attach`, `pdf.attach`, `clipboard.paste`) belong to the
  chat-basics area and are deliberately absent here.
- The gateway contract declares `UndoIntent`, `CorrectionStatus`, `ClarifyLockStatus` and
  `ForeignSource` as value enums without member lists; they are typed as `String` pass-throughs.
- `GET /api/sessions/{id}/timeline`, `/export`, `/stats`, `/empty/count` were read in the router
  but are not part of this area's scope, so they have no wrapper.
