# Hermes Desktop (Rust)

A native desktop client for [Hermes Agent](https://hermes-agent.nousresearch.com), written
100% in Rust with [Slint](https://slint.dev). No web view, no Electron, no JS.

It speaks the **existing** Hermes gateway protocol (JSON-RPC 2.0 over WebSocket, the same
contract the Electron desktop and the TUI use), so the backend is unchanged. The goal is
feature parity with the current Hermes Desktop with a restrained, Claude-Desktop-inspired
look and motion.

> **Status: early.** A working vertical slice: chat with streaming against the real
> backend, sessions sidebar, thinking blocks, tool/terminal rows, approvals, model picker,
> settings shell. Progress is tracked item by item in [docs/PARITY.md](docs/PARITY.md).

| Dark — empty | Light — empty |
|---|---|
| ![](docs/screenshots/rs-dark-empty.png) | ![](docs/screenshots/rs-light-empty.png) |
| **Chat (Verbose view, live backend)** | **Settings → Appearance** |
| ![](docs/screenshots/rs-dark-chat-verbose.png) | ![](docs/screenshots/rs-dark-settings-appearance.png) |

## Build & run

Requirements: Rust ≥ 1.88, `fontconfig`, `libxkbcommon`, `wayland`, GL/EGL; the `hermes`
CLI on `PATH` (the app attaches to the host backend, or starts `hermes serve`).

```sh
cargo run -p hermes-app --release          # binary: target/release/hermes-desktop-rs
```

Arch / Omarchy package: `cd packaging && makepkg -si` (installs the binary, a `.desktop`
entry and the icon).

### Environment

| Variable | Effect |
|---|---|
| `HERMES_DESKTOP_PORT` / `HERMES_DESKTOP_TOKEN` | attach to a specific backend |
| `HERMES_DESKTOP_ISOLATED_BACKEND=1` | always spawn a private `hermes serve` |
| `HERMES_DESKTOP_THEME=light` | system theme hint (until portal detection lands) |
| `HERMES_DESKTOP_REDUCE_MOTION=1` | disable animation |
| `RUST_LOG=hermes_desktop_rs=debug` | startup timing logs |
| `HERMES_DESKTOP_AUTOSEND`, `HERMES_DESKTOP_OPEN_SETTINGS=<page>`, `HERMES_DESKTOP_TRANSCRIPT_MODE=<0-2>` | dev hooks used for scripted screenshots |

## Development

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -p hermes-core --example probe   # live round trip; makes one tiny model call
```

Screenshots are taken in an isolated headless compositor so a running desktop session is
never disturbed:

```sh
weston --backend=headless --renderer=pixman --shell=kiosk --width=1440 --height=900 --socket=hermes-shot &
WAYLAND_DISPLAY=hermes-shot target/release/hermes-desktop-rs &
WAYLAND_DISPLAY=hermes-shot weston-screenshooter
```

Docs: [ANALYSIS](docs/ANALYSIS.md) (how the current desktop talks to the backend) ·
[ARCHITECTURE](docs/ARCHITECTURE.md) (crates, threading, why Slint) ·
[DESIGN](docs/DESIGN.md) (tokens and motion) · [PARITY](docs/PARITY.md) (feature checklist).

## Benchmarks

Measured 2026-10-16 on the dev machine (Omarchy, kernel 7.2), release profile
(`lto = "fat"`, `codegen-units = 1`, `opt-level = 3`, `strip`, `panic = "abort"`), attached
to an already-running host backend. **All numbers come from a headless Weston using the
software GL path (llvmpipe)** — scripted so the user's live session wasn't touched. A GPU
session is expected to differ (first frame, memory) but is **not measured yet**.

| Metric | Result | How |
|---|---|---|
| Binary size | 21.3 MB (stripped) | `stat target/release/hermes-desktop-rs` |
| Process start → window created | 21 ms (runs: 21.4 / 21.5 / 21.4) | `tracing` timestamps from `main` |
| Process start → **first frame rendered** | **361–374 ms** (runs: 371.6 / 373.7 / 360.7) | Slint `RenderingState::AfterRendering` |
| Idle CPU (30 s, window open, no activity) | **0.0 %** (0 ticks) | `/proc/<pid>/stat` utime+stime delta |
| Memory (idle, window open) | RSS 188 MiB · PSS 158 MiB | `/proc/<pid>/status`, `smaps_rollup` |

**Known gaps:**
* First frame is **over the 300 ms target** in this software-GL setup; ≈350 ms of it is
  between event-loop start and the first presented frame (EGL/llvmpipe init, font loading,
  first layout). Not profiled yet (`perf`/`strace` aren't installed); next steps are a GPU
  measurement and lazy-loading the serif/mono fonts.
* RSS here includes Mesa llvmpipe; re-measure under the real GPU driver.
* The Electron Hermes Desktop wasn't running during measurement, so there is no
  side-by-side memory comparison yet.

## Licences

Code: MIT. Bundled fonts (Inter, Source Serif 4, JetBrains Mono): SIL OFL 1.1, texts in
`assets/fonts/`. Icons: Lucide (ISC), `assets/icons/LICENSE-lucide.txt`. The app icon
(`packaging/hermes-desktop-rs.svg`) is original. No Anthropic assets are used; the look is
recreated from observation with openly licensed fonts and icons.
