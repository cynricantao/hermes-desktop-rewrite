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
    examples/probe.rs  live end-to-end probe against a real backend
  hermes-ui/     Slint views (ui/*.slint) + controller bridging core ↔ UI
    ui/theme.slint       design tokens (docs/DESIGN.md) + Motion/Ease globals
    ui/components.slint  buttons, chips, toggle, segmented control, spinner, shimmer, banner
    ui/chat.slint        virtualised transcript, tool groups, thinking block, composer
    ui/settings.slint    settings modal
    ui/app-window.slint  window: sidebar, title strip, empty state, overlays
    src/controller.rs    async controller (tokio) — owns client, backend, transcript
    src/rows.rs          Block → Slint row conversion (pure, tested)
  hermes-app/    binary `hermes-desktop-rs`: logging, runtime, window
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
All durations come from the `Motion` global (60/120/200/300 ms ladder from DESIGN.md
§8.1); easings from `Ease`. Setting *Reduce motion* (Settings → Appearance, or
`HERMES_DESKTOP_REDUCE_MOTION=1`) sets every duration to 0 and freezes spinners/shimmer.

| Element | Duration | Easing |
|---|---|---|
| hover / press colour | 120 ms | ease-snap `(.32,.72,0,1)` |
| tool row expand + chevron | 300 ms height / 200 ms chevron | ease-out `(.165,.84,.44,1)` |
| modal in / out | 200 / 150 ms, scale .98→1 + fade | ease-snap |
| sidebar collapse | 300 ms width | ease-out |
| toggle knob, segmented pill | 200 ms | ease-out |
| row insert | 200 ms fade | ease-out |
| spinner | 1 rev / ≈0.8 s, continuous | linear |

## Testing
* `cargo test --workspace` — reducer, discovery parsing, row conversion, helpers.
* `cargo run -p hermes-core --example probe` — live round trip against the real backend.
* Screenshots are produced headlessly (Weston headless + pixman, kiosk shell) so the
  user's own Hyprland session is never touched; see README.
