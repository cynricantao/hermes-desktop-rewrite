# Architecture

## Goals that drive the design
* **Same features as Hermes Desktop**, talking to the *unchanged* Hermes backend over its
  existing protocol (see [ANALYSIS.md](ANALYSIS.md)).
* **The UI thread never blocks.** All I/O, streaming and process management run on tokio.
* **Fast and light:** native rendering, no web view, virtualised lists.

## Workspace

```
crates/
  hermes-core/   protocol + client + pure state reducers. No UI dependency.
    backend.rs     attach-first discovery (env → spawn-ledger.json → spawn `hermes serve`)
    rpc.rs         JSON-RPC 2.0 over WebSocket: requests, events, server→client requests, heartbeat
    protocol.rs    lenient serde types mirroring gateway-contract.openrpc.json
    transcript.rs  gateway events → transcript blocks, reporting row-level `Change`s
    sessions.rs    sidebar grouping helpers
    remote.rs        Remote gateway connections: URL rules, HTTP + WebSocket auth probe, saved profile.
    artifacts.rs     Artifact detection, registry and library scan (pure).
    notify.rs        Notification rules: when to toast / notify, copy, throttling (pure).
    keybinds.rs      Shortcut registry: actions, Linux defaults, parse/match, overrides.
    flip.rs          FLIP list-motion planner (enter/move/exit, 40-row cap) + list windowing.
    ansi.rs          ANSI SGR → styled runs for terminal and tool output.
    browser.rs       In-app browser helper protocol + preview.read / preview.act mapping.
    api/            Typed backend client, one module per area (sessions, plugins, messaging,
                     voice, workspace, system, extras). Pure request builders + lenient reply
                     types; screens call these. Map: docs/BACKEND_API.md.
    capabilities.rs  Capabilities screen: REST requests + row parsers for profiles, skills,
                     toolsets, MCP servers, memory/curator and cron jobs (pure, tested)
    examples/probe.rs  live end-to-end probe against a real backend
  hermes-ui/     Slint views (ui/*.slint) + controller bridging core ↔ UI
    ui/theme.slint       design tokens (docs/DESIGN.md) + Motion/Ease globals
    ui/kit/*.slint       component kit (P0): controls, overlays, content views, sidebar
                         pieces, FLIP list item; import from ui/kit/kit.slint
    ui/components.slint  legacy primitives used by the pre-kit views
    ui/chat.slint        virtualised transcript, tool groups, thinking block, composer
    ui/settings.slint    settings modal
    ui/capabilities.slint  Capabilities modal (six tabs, filter, create form, detail editor)
    ui/app-window.slint  window: sidebar, title strip, empty state, overlays
    src/controller.rs    async controller (tokio) — owns client, backend, transcript
    src/rows.rs          Block → Slint row conversion (pure, tested)
    src/capabilities.rs  Capabilities task: its own tokio task, so a slow MCP test or a
                         cron "run now" never stalls chat streaming
  hermes-app/    binary `hermes-desktop-rs`: logging, runtime, window
  hermes-gallery/ dev tool: kit gallery window, headless board renderer, golden test
                  (goldens/<os>/*.png). Never part of the shipped binary.
assets/          bundled fonts (OFL) and icons (ISC) with their licences
packaging/       PKGBUILD, .desktop file, icon
```

## Data flow

```
          UI thread (Slint event loop)                      tokio workers (2)
 ┌─────────────────────────────────────┐   Cmd (unbounded mpsc)   ┌────────────────────────────┐
 │ callbacks: send / stop / open / …   │ ───────────────────────▶ │ Controller                 │
 │                                     │                          │  ├ Client (rpc.rs)  ◀──WS──▶ hermes serve
 │ AppState global (rows, sessions,…)  │ ◀─────────────────────── │  ├ Transcript reducer       │
 └─────────────────────────────────────┘ invoke_from_event_loop   │  └ ManagedBackend (child)   │
                                          (row patches, ≤1/frame) └────────────────────────────┘
```

1. A UI callback only enqueues a `Cmd`; it never awaits.
2. The controller applies gateway events to `Transcript`, which returns
   `Change::{Updated(i), Inserted(i), Reset}`.
3. Dirty indices are coalesced and flushed at most once per 8 ms (≈ one 120 Hz frame).
   Only the changed rows are rebuilt and patched into the `VecModel` with `set_row_data`,
   so a long conversation is never re-laid-out because of one streaming token.
4. Transcript blocks are `Send`; Slint row structs (which hold `Rc` models) are built on
   the UI thread inside the `invoke_from_event_loop` closure.

## Backend discovery (mirrors the Electron app)
1. `HERMES_DESKTOP_PORT` (+ `HERMES_DESKTOP_TOKEN`) → attach explicitly.
2. `$HERMES_HOME/spawn-ledger.json`: newest live `serve`/`dashboard` record on a loopback
   host that is not `isolated`, validated by fetching the session token (a record alone is
   never trusted). There is one backend per host; attaching avoids a second Python process.
3. Otherwise spawn `hermes serve --host 127.0.0.1 --port 0`, wait for
   `HERMES_BACKEND_READY port=<n>`; if another launcher won the race the CLI prints
   `already running on this host … port <n>` and we attach to that instead.
   `HERMES_DESKTOP_ISOLATED_BACKEND=1` skips step 2.
An owned backend is killed when the app exits (`kill_on_drop`).

## Why Slint (and not iced)
Spike result (this repo, Slint 1.18, winit + FemtoVG, Wayland):

| Criterion | Slint | iced |
|---|---|---|
| Declarative property animations (`animate x { duration; easing }`) with cubic-bezier easing | built in — every DESIGN.md easing maps 1:1 | hand-rolled per widget via subscriptions/time |
| Continuous animation (spinner, shimmer) | `animation-tick()`; only redraws while visible | manual frame subscription |
| Virtualised list | `ListView` instantiates only visible rows | `scrollable` + manual windowing (lazy widgets) |
| Rich text | `StyledText` (markdown subset) built in | needs a markdown→widget layer |
| Accessibility | AccessKit roles/labels on every element | partial |
| Wayland | native via winit | native via winit |

Measured in the spike: release cold start to first frame and idle numbers are in the
README *Benchmarks* section. Slint's declarative animation model is what makes the
DESIGN.md motion table cheap to implement and to keep consistent; iced would need a
custom animation layer for every component. egui was excluded by the brief.

Trade-offs accepted: Slint's `StyledText` covers a CommonMark subset (no tables, no
syntax highlighting) — code highlighting and tables will be rendered as dedicated row
components; Slint is GPLv3 / royalty-free / commercial licensed (we use it under the
royalty-free desktop licence terms; see Slint's LICENSE).

## Motion
All durations come from the `Motion` global and easings from `Ease` (docs/DESIGN.md §5):
150 ms ease-out for hover/press and the FLIP list motion, 120 ms message enter, 150 ms
toast, `Motion.panel` (device setting 0–400 ms, default 180 ms) with
`Ease.drawer = cubic-bezier(0.32, 0.72, 0, 1)` for panels, 200/150 ms dialogs. Setting
*Reduce motion* (Settings → Appearance, or `HERMES_DESKTOP_REDUCE_MOTION=1`) sets every
duration to 0 and freezes spinners/shimmer. Only translate + opacity animate (the default
software renderer ignores rotate/scale and draws no shadows; DESIGN.md §6).

## Testing
* `cargo test --workspace` — reducer, discovery parsing, row conversion, helpers.
* `cargo run -p hermes-core --example probe` — live round trip against the real backend.
* Screenshots are produced headlessly (Weston headless + pixman, kiosk shell) so the
  user's own Hyprland session is never touched; see README.

## In-app browser helper

`helpers/hermes-browser` is a separate GTK + wry (WebKitGTK) program, excluded from the workspace and started only when a page is opened. The app drives it over line-delimited JSON on stdin/stdout (`hermes-core::browser`); `crates/hermes-ui/src/browser.rs` owns it on its own task. Linking WebKit into the app measured +46 ms first frame and +20 MiB, hence the separate process. Details: docs/browser.md.
