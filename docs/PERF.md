# Startup and memory profile

Measured 2026-10-16 on the dev machine (Omarchy, kernel 7.2, 12 threads, NVIDIA driver
610.57 + Mesa 26.2.2). Release profile. Attached to an already-running host backend.

> **All numbers below come from a private headless Weston (`--renderer=pixman`).** That
> compositor has no GPU. The app's EGL there ends up on Mesa's **software** GL (llvmpipe),
> after first trying the NVIDIA EGL vendor. Your real Hyprland/NVIDIA session will behave
> differently and **has not been measured**. Run this yourself and paste the output:
>
> ```sh
> cd ~/Projects/hermes\ desktop && cargo build --release -p hermes-app && HERMES_MEASURE_LIVE=1 scripts/measure-startup.sh 5 && HERMES_MEASURE_LIVE=1 scripts/measure-memory.sh
> ```
>
> Each run briefly opens a window and closes it after its first frame.

## Method

| Tool | What it measures |
|---|---|
| `scripts/measure-startup.sh [runs]` | `tracing` marks on target `startup` (`crates/hermes-ui/src/perf.rs`): process start → each phase → first frame (Slint `RenderingState::AfterRendering`). The app quits after the first frame (`HERMES_DESKTOP_EXIT_AFTER_FIRST_FRAME=1`). |
| `scripts/first-commit.py <bin> [runs]` | Process start → first `wl_surface.commit` that carries a buffer, read from `WAYLAND_DEBUG=client`. Works for any renderer, including ones without a rendering notifier. |
| `scripts/gdb-startup-trace.py` | Entry→return wall time of fontconfig, fontique and EGL calls (gdb `FinishBreakpoint`). gdb inflates absolute times roughly 10–20×, so read only the **ratios**. |
| `scripts/sample-startup.py` | `eu-stack` sampling of the main thread (no `perf` installed, and `ptrace_scope=1` is handled with `PR_SET_PTRACER`). Needs `cargo build --profile profiling`. |
| `scripts/measure-memory.sh` | RSS/PSS from `/proc/<pid>/smaps_rollup` 3 s after first frame, plus the GL driver libraries that are mapped. |
| `scripts/headless.sh` | Private Weston wrapper used by all of the above. It unsets `DISPLAY` and owns its own `WAYLAND_DISPLAY` socket. |

## Before: where the first frame goes (femtovg / OpenGL renderer)

`scripts/measure-startup.sh 10`, mean of each phase over 10 runs:

| ms from `main` | Phase |
|---:|---|
| 0.0 | logging ready |
| 0.2 | tokio runtime built |
| 22.0 | window component created (Slint component tree + bundled fonts registered) |
| 22.1 | controller started, `window.show()` returned |
| 23.9 | *(tokio)* backend discovered |
| 25.7 | *(tokio)* websocket connected |
| 35.8 | *(tokio)* session list fetched |
| 107.5 | *(tokio)* model label fetched |
| **315.2** | **renderer setup (EGL display + GL context)** |
| 317.7 | first frame: before render |
| **354.6** | **first frame presented** |

First frame over 10 runs: **median 347.4 ms**, min 335.7, max 432.1.

Breakdown:

| Bucket | Cost | Evidence |
|---|---:|---|
| Our code before the event loop (component, theme, fonts, controller spawn) | **~22 ms** | marks above |
| Backend discovery, websocket, session list, model label | **0 ms on the critical path** | Runs on the tokio workers, in parallel with GL init. With the backend unreachable (`HERMES_DESKTOP_PORT=1`), first frame was 351–392 ms, the same as with it. |
| Fonts (fontconfig init + 4 fallback sorts + fontique registration) | **≈ 2–3 % of the GL-init cost** | gdb ratios: `FcInitLoadConfig` 6.6, `FcFontSort` 4×≈1, `register_fonts` 3×<0.5, against `eglInitialize` + `eglCreateContext` ≈ 13 000 in the same (gdb-slowed) units |
| **EGL/GL driver init** | **~290 ms** | Between `window shown` (22 ms) and `renderer setup` (315 ms). gdb: nearly all of it is in `eglInitialize` and `eglCreateContext`. |
| First render (layout, glyph atlas, shader compile, swap) | ~37 ms | `before render` → `first frame` |

Why GL init is slow here: glvnd loads **both** EGL vendors (`10_nvidia.json` first, then
`50_mesa.json`). Both get mapped (`measure-memory.sh` lists `libEGL_nvidia`,
`libnvidia-eglcore`, `libEGL_mesa`, `libgallium`, `libLLVM`). NVIDIA can't serve the pixman
compositor, so Mesa falls back to llvmpipe (`MESA-EGL: failed to get driver name for fd -1`),
which JIT-compiles its shaders through LLVM. Controlled experiments, 4 runs each:

| Variant (env only, same binary) | renderer setup | first frame |
|---|---:|---:|
| default | 315–393 ms | 355–435 ms |
| `__EGL_VENDOR_LIBRARY_FILENAMES=…/50_mesa.json` (skip the NVIDIA probe) | 124–138 ms | 166–188 ms |
| …plus `GALLIUM_DRIVER=llvmpipe` (skip the zink/kopper probe) | 76–85 ms | 117–136 ms |

So roughly 190 ms is the NVIDIA vendor probe that fails, about 50 ms is Mesa's driver probing,
and the rest is llvmpipe itself. None of it is in our code, and none of it says anything
about your real GPU, where the NVIDIA vendor should succeed on the first try.

### What the requested fix would and wouldn't buy

The brief asked to show the shell first and defer fonts, assets, the backend handshake and the
session list. The data says that would save almost nothing:

* The backend handshake and session list already run off the UI thread and finish before GL
  is up.
* Fonts and theme cost a few ms in total.
* The ~290 ms goes into driver init, which has to finish before *any* GL frame (even an empty
  skeleton) can be shown.

## The lever that does work: Slint's software renderer

Same binary built with `renderer-software` enabled next to `renderer-femtovg` (no new
third-party crates, since the renderer is part of Slint; binary 21.3 → 22.3 MB). Selected with
`SLINT_BACKEND`, measured by first buffer commit (`first-commit.py`, 8 runs each):

| Renderer | First buffer commit | PSS 3 s after first frame | RSS |
|---|---:|---:|---:|
| `winit-femtovg` (current) | median **348.2 ms** (333–412) | **161.7 MiB** | 200 MiB |
| `winit-software` | median **49.4 ms** (46–51) | **24.6 MiB** | 35 MiB |

Both rendered the same UI correctly in the headless screenshots (`scripts/shot.sh`). The
software render looked identical at 1440×900: same fonts, icons, radii and borders.

**Not adopted yet. It's a renderer decision, so it needs your call.** Open questions:

* CPU cost of software rendering while streaming, scrolling long transcripts, at HiDPI scale,
  and during animations. Not measured yet.
* How femtovg does on your real NVIDIA session. If it's already well under 300 ms there, the
  case for switching is mostly memory.

## Memory: the 158 MiB figure

`measure-memory.sh`, femtovg, idle on the welcome screen with ~20 sessions in the sidebar:
**PSS 159–162 MiB** (RSS ~200 MiB). Split:

| Part | PSS | What it is |
|---|---:|---|
| File-backed | ~90 MiB | Mostly mapped GL driver code: `libLLVM` (llvmpipe JIT), `libgallium`, and the NVIDIA EGL libraries loaded by the failed probe. Plus our 21 MB binary. |
| Anonymous | ~66 MiB | Heap: llvmpipe's state and render targets, the glyph atlas, the Slint tree, tokio. |
| Shmem | ~5 MiB | Wayland buffers |

**Baseline without GL:** the same UI on the software renderer is **PSS 24.6 MiB** (5.5 MiB
anonymous). So about **135 MiB of the 158 MiB is the GL stack on this display**, not the app.

**What scales with conversation size:** transcript rows (`rows.rs`: one `ChatRow` per
message or tool call holding text and markdown runs) and their Slint item trees. Not measured
yet against a long conversation. That needs a seeded long session in the headless display.

**What can be trimmed:**
1. The software renderer: −135 MiB on this display (pending the decision above).
2. On GL, skipping the failing vendor probe saves the NVIDIA libraries' mappings. This has to
   be done by the launcher or the environment, not by us.
3. Transcript windowing (PARITY item "Transcript windowing / retention") bounds the part that
   grows with conversation size.

## After

No startup fix has been applied yet. The two candidate changes and their headless numbers are
in the table above. The "after" measurement will be filled in once a renderer decision is made
and your GPU numbers are in.
