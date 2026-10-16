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

## Your real GPU session (femtovg, run by you, 2026-10-16)

`HERMES_MEASURE_LIVE=1 scripts/measure-startup.sh 5` on Hyprland + NVIDIA 610.57:

| | Result |
|---|---|
| First frame | **median 122.2 ms** (120.5 / 121.5 / 122.2 / 150.3 / 162.7) |
| Window component created | 23–29 ms |
| Renderer setup (GL context) | 101–143 ms |
| Memory 3 s after first frame | **PSS 79 MiB** (anon 23 MiB, file 56 MiB), RSS 111 MiB |

So on real hardware femtovg already meets the 300 ms target. About 80 ms of the 122 ms is
still NVIDIA EGL init, and most of the PSS is mapped driver code (both NVIDIA and Mesa EGL
are loaded there too).

## After: software renderer by default (decided 2026-10-16)

`renderer-software` is now compiled in next to femtovg. `main.rs::select_renderer()` picks
software first and falls back to femtovg if it can't initialise.
`HERMES_DESKTOP_RENDERER=gl` reverses that order, and `SLINT_BACKEND` overrides both.
Binary: 21.3 → 22.3 MB.

Hidden display, same binary, `first-commit.py` 10 runs each:

| Renderer | First buffer commit | PSS idle |
|---|---:|---:|
| software (new default) | median **49.3 ms** (46.7–54.1) | **23.9 MiB** |
| `HERMES_DESKTOP_RENDERER=gl` | median 351.5 ms (332.5–418.8) | ~160 MiB |

**CPU while a real reply streams** (`scripts/measure-stream-cpu.sh 25`, one prompt to the
real backend, utime+stime over 25 s, % of one core):

| Display | software | gl (llvmpipe here) |
|---|---:|---:|
| 1440×900, scale 1 | **5.8 %**, peak RSS 36.7 MiB | 87.9 %, peak RSS 208.9 MiB |
| 1440×900, scale 2 (HiDPI) | **7.0 %**, peak RSS 66.8 MiB | 225 %, peak RSS 275.7 MiB |

The GL column is llvmpipe, which renders on the CPU, so it is **not** a fair stand-in for
your GPU. The software column is representative, because it does the same work everywhere.
A mid-stream screenshot under the software renderer showed the reply streaming correctly.

**Software renderer on the user's real Hyprland session** (run by the user 2026-10-16, pasted
output; release build of `c616c4a`):

| Metric | Value |
|---|---|
| First buffer commit, `first-commit.py` ×5 | 48.7, 33.1, 32.9, 31.6, 31.6 ms → **median 32.9 ms** (min 31.6, max 48.7) |
| Idle memory, `measure-memory.sh` | **RSS 30.1 MiB, PSS 22.3 MiB** (Pss_Anon 6.0 MiB, Pss_File 14.5 MiB, Pss_Shmem 1.8 MiB, Private_Dirty 6.0 MiB) |
| GL/driver libraries mapped | none (software renderer, as expected) |

That is well inside the 300 ms first-frame budget and lower than the femtovg/GPU figures
above. `measure-memory.sh` printed "(no first frame seen)" because the software renderer
has no rendering notifier. The memory sample is still taken after the settle time.

Animation-heavy frames on the software renderer have not been measured yet. To repeat the
real-session measurement (it opens and closes a window 5 times, then once more for the
memory figure):

```sh
cd ~/Projects/hermes\ desktop && git pull && ~/.cargo/bin/cargo build --release -p hermes-app && scripts/first-commit.py target/release/hermes-desktop-rs 5 && HERMES_MEASURE_LIVE=1 scripts/measure-memory.sh
```

(`measure-startup.sh` only reports a first frame for femtovg, because the software renderer
has no rendering notifier. `first-commit.py` measures both from the Wayland protocol.)

## Long conversation (600 messages), software renderer

Seeded with `scripts/seed-long-session.py 600` into the private test backend (no model
calls), opened in the app, then swept to the top and back once.

**In-process** (`tests/live_backend.rs`, `HERMES_LIVE_ONLY=long`, Slint testing backend):
600 rows loaded in 100 ms; **6 `MessageRow` instances alive at rest, at most 7 during the
sweep** (the ListView instantiates only visible rows). RSS: 24.3 MiB before opening,
27.0 MiB loaded, 27.1 MiB after sweep 1, 27.1 MiB after sweep 2. In the final full-suite run
(after the other sections had run) it was 29.3, 30.7, 30.5 and 30.5 MiB.

**Release binary, hidden display** (`scripts/measure-long-transcript.sh <id>`):

| Window | 1440×900, scale 1 | 1440×900, scale 2 |
|---|---:|---:|
| idle, session loaded (4 s) | 0.0 % CPU · RSS 36.4 MiB | 0.2 % · 65.9 MiB |
| sweep top → bottom (12 s, ~7500 px/s) | 12.0 % · 36.5 MiB | 23.9 % · 66.0 MiB |
| idle after the sweep (5 s) | 0.2 % · 36.5 MiB | 0.2 % · 66.0 MiB |

So virtualisation holds, memory stays flat, and CPU returns to ~0 when scrolling stops.
The sweep is far faster than a human scrolls, so 12 % is an upper bound. Not measured
on your real session: it needs a private backend port, so it can't point at your sessions.
