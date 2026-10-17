# System widgets — parity notes (`system` area)

Area module: `crates/hermes-ui/src/widgets/system.rs` · view: `crates/hermes-ui/ui/w-system.slint`
Worktree: `~/Projects/hermes desktop-wid-system` (branch `rewrite/wid-system`).
Nine ids, all the `system` area's registry rows. For the parent to merge into `docs/PARITY.md`.

## Ids

| id | registry page | state | endpoint(s) |
|---|---|---|---|
| `about.version` | About › Version & updates | implemented | `GET /api/status`, `GET /api/hermes/update/check`, `POST /api/hermes/update` |
| `sessions.archived-list` | Sessions › Archive & retention | implemented | `GET /api/sessions?archived=only`, `GET /api/sessions/stats`, `GET /api/sessions/empty/count`, `{GET mutation-snapshot, PATCH}` / `{GET mutation-snapshot, DELETE}` `/api/sessions/{id}`, `DELETE /api/sessions/empty`, `POST /api/sessions/prune` |
| `gateway.mode` | Gateways › This window | implemented | `GET /api/status`, `GET /api/host/identity`, `POST /api/gateway/restart` |
| `plugins.list` | Plugins › Desktop plugins | implemented | `GET /api/dashboard/plugins`, `GET /api/dashboard/plugins/rescan`, `POST /api/dashboard/plugins/{name}/visibility`, `GET /api/config` (hidden list) |
| `billing.overview` | Billing › Overview | implemented | `GET /api/portal`, `GET /api/analytics/usage?days=30` |
| `keybinds.list` | Keyboard Shortcuts › Key bindings | implemented | none (this client's own bindings) |
| `about.uninstall` | About › Uninstall | implemented | none (local paths only) |
| `gateway.devices` | Gateways › Saved connections | implemented (honest scope, see below) | none live; `prefs.json` only |
| `gateway.updates` | Gateways › Remote updates | implemented as an honest note | none |

## What the backend actually offers (checked against the source, not guessed)

* **Archived list.** Electron's Archived page pages `/api/profiles/sessions?archived=only`
  across every profile; the Rust panel stays inside the settings "Applies to" scope and uses
  `GET /api/sessions?archived=only&order=recent&limit=100` with `?profile=` — one store, the
  one the page is scoped to.
* **Restore / delete are fenced.** `hermes_cli/web_server_sessions.py::_mutate_session_request`
  answers `409 mutation_identity_required` unless `request_id` + `expected_revision` are
  present, and DELETE takes them as query params. So the widget reads
  `GET /api/sessions/{id}/mutation-snapshot` first and carries
  `runtime_revision`/`runtime_generation` into the write — the same fencing
  `apps/shared/src/session-http-mutations.ts` does in Electron. This is the one write path in
  the area that needs a two-step read.
* **Cleanup.** `DELETE /api/sessions/empty` deletes sessions with no messages; `POST
  /api/sessions/prune` deletes ended sessions (default `older_than_days: 90`). The button does a
  `dry_run: true` POST first and reports `matched`/`skipped_open`, then deletes on the second
  click — so the panel never deletes something the user has not seen counted.
* **Gateway.** `GET /api/status` (`version`, `release_date`, `gateway_running`, `gateway_state`)
  and `GET /api/host/identity` (`protocolVersion`, `pid`, `role`); restart is `POST
  /api/gateway/restart` (profile-scoped, no body).
* **Updates.** `GET /api/hermes/update/check` returns `install_method`, `current_version`,
  `behind`, `update_available`, `can_apply`, `update_command`, `message`; `POST
  /api/hermes/update` applies it. Apply is two-click and disabled while `can_apply` is false.
* **Plugins.** `GET /api/dashboard/plugins` returns installed desktop plugins **after** filtering
  out hidden and non-enabled ones, so it cannot report a hidden plugin's own state. The hidden
  set lives in the profile config (`dashboard.hidden_plugins`), which is exactly what
  `POST /api/dashboard/plugins/{name}/visibility {hidden}` writes; the panel reads that config key
  and renders hidden plugins as extra rows so "show" is possible. The toggle therefore writes
  **visibility**, not an enable/disable flag.
* **Billing.** `GET /api/portal` (`logged_in`, `portal_url`, `subscription_url`, `account_tier`,
  `free_tier`, `features[]`) and `GET /api/analytics/usage?days=30` (`totals`, `by_model[]`,
  `period_days`). Read-only by design: the original's charge/step-up/auto-reload flows are
  gateway-RPC billing mutations that this client does not implement, and the portal owns them.

## Honest scoping (features the original has that this client does not)

* **`gateway.devices` is not a device manager.** The original manages SSH installs in a
  connection registry (`store/connections`, `connections-registry.tsx`) and launches the app
  against them. This client attaches with `HERMES_DESKTOP_PORT` or local discovery and has no SSH
  transport, so the panel states the current endpoint, gives the copyable launch command for
  pointing the app at another backend, and keeps a small **saved-connections list in
  `prefs.json`** (`hermes-desktop-saved-connections`: `{id, label, host, port}`). "Command" shows
  the launch line for that entry; "Forget" drops it. Nothing here switches the live connection —
  saving a connection does not connect to it.
* **`gateway.updates` is a note.** `managed-updates-section.tsx` drives a transactional SSH update
  engine for registered SSH installs, and renders nothing at all when there are none. The Rust
  panel says so in one paragraph and points at About › Version & updates for the local backend.
* **`keybinds.list` is read-only.** The bindings rendered are read out of
  `ui/app-window.slint` (`FocusScope.capture-key-pressed`) and `ui/chat.slint` (composer):
  Ctrl+N, Ctrl+,, Ctrl+B, Ctrl+Q, Ctrl+Tab / Ctrl+Shift+Tab (+Backtab), Esc (close overlay / deny
  approval / stop a running reply), Enter (send, or allow-once on an empty composer), Shift+Enter.
  There is no command palette and no Ctrl+K. Per-user rebinding (the original's
  `store/keybinds` + click-to-record) is not implemented; the panel says so.
* **`about.uninstall` takes no action.** It reports the real paths on this machine (running
  binary from `current_exe`, `~/.local/bin/hermes-desktop-rs`, the `.desktop` entry,
  `/usr/bin/hermes-desktop-rs` when packaged, the checkout) with a copyable removal command, and
  states that it does not touch the Hermes backend (`hermes uninstall` is the user's call).
* **`plugins.list` sub-pages are out of scope.** The original opens a plugin-provided settings
  page per plugin (`contrib/settings-pages`); one line in the panel notes this, and only the
  registry row is rendered.

## Known limitations / verification gaps

* Saved connections are written to `prefs.json` directly from the widget. The controller keeps its
  own in-memory `Prefs` snapshot; a later controller-side device-pref write could in principle
  rewrite the file from that snapshot and drop the saved-connections key. Both paths
  load-modify-write the whole file, so the window is small, but it is a real race — moving the
  list into the controller's `Prefs` would close it.
* `Apply update` was **not** exercised against a backend: triggering `/api/hermes/update` even on
  the private test backend would rewrite that checkout's Hermes install, which AGENTS.md forbids
  ("never trigger /api/hermes/update"). Its two-click gate, disabled state and payload were
  verified by rendering the panel against the live private backend; the POST itself is
  code-reviewed only.
* `sessions.archived-list` restore/delete were exercised against the private test backend with a
  seeded archived session (see the verification log in the PR/report); the `mutation-snapshot`
  precondition is a hard requirement of the backend, so a stale read surfaces as a 409 inline
  error rather than a silent failure.
