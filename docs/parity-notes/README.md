# Settings widgets — build-out brief (for the four area agents)

Base branch: `rewrite/settings-live` (worktree `~/Projects/hermes desktop-settings-live`).
The panel shell, dispatch, controller routing and one file per area already exist. Each
agent owns **one Rust module + one Slint view file** and must not edit anything else.

## The contract (already wired, do not change)

* Row rendering: `crates/hermes-ui/ui/settings.slint` draws a widget row as label +
  description on top and the area's component below, full width. The component to use
  comes from `SettingRowUI.area` (set from `crates/hermes-ui/src/widgets/mod.rs::area`).
* Loading: the controller calls `widgets::load(id, &ctx)` for every owned id on the open
  page after each publish (`publish_settings` → `load_widgets`), and again after every
  edit, scope change, page change and `Outcome::Reload`.
* Actions: a widget reports clicks only as `act("<area>:<verb>[:<arg>]")`
  (settings.slint wires `act` → `SettingsState.action`). The controller routes
  `<area>:…` to your `action()`; anything that is not your area's prefix is ignored.
* `Ctx` (`crates/hermes-ui/src/widgets/mod.rs`) gives you: `get_json`, `post_json`,
  `put_json`, `delete_json`, `config()` (scoped profile config), `config_set(dotted,
  value)` (same sparse patch the generic rows use), `profile_query()` and `ui(f)` to run a
  closure on the UI thread. Your module fills **your own** Slint global; it never touches
  `SettingsState`.
* Return `Outcome::{None, Notice, Error, Edit, Reload}`:
  * `Edit(id, value)` when a widget control writes a plain registry setting — the panel's
    own edit path then saves it (with its verification and side effects, e.g. fonts).
  * `Reload` when the profile config changed (rows on the page depend on it).
  * `Notice`/`Error` for inline feedback (the panel renders it above the rows).

## Hard rules (repo: AGENTS.md)

* Work only in your own worktree/branch. Never touch `settings.slint`, `controller.rs`,
  `crates/hermes-core/**`, another area's files, or `docs/PARITY.md` (write your parity
  notes to `docs/parity-notes/<area>.md` instead — new file, yours).
* Display: never start the app on the user's desktop; use `scripts/headless.sh` /
  `scripts/shot.sh` only. Never point the app at the real backend: live checks use a
  private backend (`scripts/live-test-backend.sh` with your own `HERMES_LIVE_HOME` and an
  explicit `HERMES_DESKTOP_PORT`).
* No new crates or system packages. Everything you need is already in the workspace.
* Verify with real output before claiming anything: `cargo fmt --all`,
  `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, then
  a live check against a private backend (an automated test where practical — extend
  `crates/hermes-ui/tests/live_backend.rs` only if your area fits its harness; otherwise
  prove it with a headless screenshot + a curl-level check of the endpoint you used).

## What exists today

`GET /api/status` (version etc.), `/api/config?profile=`, `/api/config/schema`,
`/api/config/defaults`, `POST /api/desktop/notify`. The panel already renders every
non-widget row (toggles/selects/text/number/list/slider) and writes them to the profile
config or `prefs.json`. Payload captures for every endpoint the areas use are in
`~/.hermes/cache/scratch/settings-api/*.json` (real responses from a private test
backend), and the OpenAPI listing is at
`~/.hermes/cache/scratch/hermes-openapi.json` (301 routes) — prefer it over guessing.

The original Electron implementation for each panel is the design reference:
`~/.hermes/hermes-agent/apps/desktop/src/app/settings/<file>.tsx` (per-area files are
named in each area brief). Mirror its behaviour, not its pixels — our visual language is
`crates/hermes-ui/ui/theme.slint` + `crates/hermes-ui/ui/components.slint`; reuse those
components (`Toggle`, `Segmented`, `PrimaryButton`, `GhostButton`, `Chip`, `IconButton`,
`Icons.get(name)`), keep rows quiet and dense, no new motion types beyond
`Motion.*`/`Ease.*`.

## Headless screenshot recipe

```bash
# in your worktree, with your own private backend running and a scratch prefs file
HERMES_LIVE_HOME=~/.cache/hermes-desktop-rs/live-<area> scripts/live-test-backend.sh &
PORT=<printed port>
HERMES_DESKTOP_PORT=$PORT HERMES_DESKTOP_PREFS=<your scratch>/prefs.json \
HERMES_DESKTOP_OPEN_SETTINGS=<group>/<page> \
scripts/shot.sh docs/screenshots/rs-<area>-<page>.png 5 target/debug/hermes-desktop-rs
```

`HERMES_DESKTOP_OPEN_SETTINGS` opens the panel directly on that registry page (it goes
through the controller, so widgets load for real). Commit screenshots under
`docs/screenshots/` with your area in the name.
