# AGENTS.md — working on Hermes Desktop (Rust)

Read this before touching the repo. It applies to every agent (Claude Code, Codex, Hermes
workers, …) and to humans.

## 1. What this is

A native **Rust** rewrite of Hermes Desktop with a **Slint** UI. The design is inspired by
Claude Desktop (restrained, warm, serif greeting, quiet motion), recreated from observation
with openly licensed fonts and icons. The goal is **the same features as the current
Hermes Desktop** (`apps/desktop`, Electron/React), talking to the **unchanged** Hermes
backend over its existing JSON-RPC 2.0 WebSocket gateway protocol.

Key docs: [README](README.md) · [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) ·
[docs/DESIGN.md](docs/DESIGN.md) · [docs/PARITY.md](docs/PARITY.md) (the feature checklist,
source of truth for progress) · [docs/PERF.md](docs/PERF.md) · [docs/ANALYSIS.md](docs/ANALYSIS.md).

## 2. Crate layout — what belongs where

| Crate | Contents | Must not contain |
|---|---|---|
| `crates/hermes-core` | Gateway client (`rpc.rs`), backend discovery/spawn (`backend.rs`), lenient protocol types (`protocol.rs`), pure state reducers (`transcript.rs`, `sessions.rs`), pure helpers (markdown/diff parsing, config logic). Unit-tested. | **Any UI code.** No `slint` dependency, ever. |
| `crates/hermes-ui` | Slint views (`ui/*.slint`), the async controller bridging core ↔ UI (`src/controller.rs`), Block → row conversion (`src/rows.rs`), the live end-to-end test (`tests/live_backend.rs`). | Protocol parsing or reducers that could live in core. |
| `crates/hermes-app` | The `hermes-desktop-rs` binary: logging, tokio runtime, renderer selection, window. Thin. | Feature logic. |

Rules of thumb: if it can be tested without a window, it goes in `hermes-core`. The UI
thread never awaits: callbacks enqueue a `Cmd`; the controller patches rows via
`invoke_from_event_loop` at most once per frame.

## 3. Commands

Rust is installed via rustup; if `cargo` is not on `PATH`, use `~/.cargo/bin/cargo`.

| What | Command |
|---|---|
| Build (release) | `cargo build --release -p hermes-app` → `target/release/hermes-desktop-rs` |
| Run (headless only, see §4) | `scripts/headless.sh target/release/hermes-desktop-rs` |
| Screenshot (headless) | `scripts/shot.sh out.png [settle-s]` |
| Format | `cargo fmt --all` |
| Lint | `cargo clippy --workspace --all-targets -- -D warnings` |
| Unit tests | `cargo test --workspace` |
| Live backend test | terminal 1: `scripts/live-test-backend.sh` (runs until killed; the backend prints its port) · terminal 2: `HERMES_DESKTOP_PORT=<port> cargo test -p hermes-ui --test live_backend -- --ignored --nocapture` |
| First frame | `scripts/first-commit.py target/release/hermes-desktop-rs 10` (headless by default) |
| Startup phases (femtovg only) | `scripts/measure-startup.sh 10` |
| Memory | `scripts/measure-memory.sh` |
| CPU while streaming | `HERMES_DESKTOP_PORT=<private port> scripts/measure-stream-cpu.sh 25` |
| Package (Arch) | `cd packaging && makepkg -s` |

**Before every commit:** `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`. All three must pass.

## 4. Hard rules

1. **Hidden display only.** Never start the app on the user's live desktop / Hyprland
   session and never screenshot the real screen. Use `scripts/headless.sh` (private Weston,
   pixman) or the Slint testing backend (`i-slint-backend-testing`, no window at all).
   Nothing may point `WAYLAND_DISPLAY`/`DISPLAY` at the user's session. If a measurement
   needs the real GPU or session, give the user **one exact command** to run and wait for
   the pasted output. Never block other work on it.
2. **Never touch the user's real backend, config or session list.** No test, script or
   probe may run against the everyday backend, `~/.hermes/config.yaml`, or the real session
   store. Live tests run only against `scripts/live-test-backend.sh` (private `HERMES_HOME`,
   `--isolated`), always with an explicit `HERMES_DESKTOP_PORT`. Do not create, rename,
   archive or delete real sessions. Never read or print secrets (`.env`, `auth.json`).
3. **No fake results.** Report only what you actually ran, with the real output. Say
   explicitly what passed, what failed, and what could not run and why. Never present a
   compile, a mock or a unit test as a live verification.
4. **Ask before adding any dependency** (crate, system package, bundled asset) **or changing
   the GUI framework/renderer.** Include the measured binary-size delta and startup cost
   in the request. Experiments go on a throwaway branch that is never merged.
5. Update `docs/PARITY.md` after every batch so its counts stay truthful.

## 5. Git workflow

* One task branch per task, created from the **freshly fetched** default branch
  (`git fetch origin && git switch -c <task> origin/main`). While the rewrite is a stack of
  unmerged PRs, branch from the tip of the stack the task depends on and say so in the PR.
* Never commit to or edit `main`. Never merge, force-push, rewrite published history, or
  bypass checks/hooks (`--no-verify`). The user merges.
* Stage named paths only (no `git add -A` when other work may be present).
* Open **draft** PRs. Delete a task branch only after the user has merged it.

## 6. Pull request requirements

Every PR description contains:

1. **What changed and why.**
2. **Files changed** (high level).
3. **Tests run, with the actual results** (counts, pass/fail, the live-test log lines).
4. **Screenshots for UI changes**, taken headlessly (`scripts/shot.sh`), committed under
   `docs/screenshots/` or attached.
5. **Known limitations** and anything not verified.
6. The **PARITY.md items claimed** by this PR and the new counts (done / partial / not started).

## 7. Coordination between agents

* Before starting: `gh pr list --state open` and read the open PRs' descriptions. Claim the
  PARITY.md items you will build in your PR description (open the draft PR early) so two
  agents never build the same thing. If an item is already claimed, pick another.
* Keep PRs small and focused (one feature area).
* On conflicts: rebase onto the updated base and resolve by keeping both sides' intent.
  Never overwrite or revert another agent's work. If the checkout is shared, check for
  other live sessions (`/proc/*/cwd`) before switching branches.

## 8. Performance budget

| Metric | Budget | Current (hidden display, software renderer) |
|---|---|---|
| First frame | **< 300 ms** | ~49 ms median (`first-commit.py`) |
| Idle CPU (window open, nothing happening) | **0 %** | 0 ticks over 20 s |
| Idle memory | stay near the PERF.md figure | PSS ~24 MiB, RSS ~35 MiB |
| CPU while a reply streams | no regression | 5.8 % of one core (1×), 7.0 % (2×) |

Any change that regresses one of these must say so in the PR with before/after numbers.
Continuous animations must stop when not visible (idle CPU 0 %).

## 9. Design rules

* Follow [docs/DESIGN.md](docs/DESIGN.md): tokens live in `ui/theme.slint`; durations come
  from the `Motion` global, easings from `Ease`.
* Only the approved animation set (DESIGN.md §8 / ARCHITECTURE.md *Motion*). No new kinds
  of motion without approval.
* Respect **reduce motion** (`Motion.reduce`, `HERMES_DESKTOP_REDUCE_MOTION=1`): every
  duration 0, spinners/shimmers frozen.
* **No proprietary Anthropic assets** (fonts, icons, logos, illustrations). Use the bundled
  OFL fonts and Lucide icons; record licences for anything new.

## 10. Dependencies

Ask the user first. The request states: the crate and why, alternatives considered, the
**release binary size delta** and **startup cost** measured on a throwaway branch, and the
memory impact.

## 11. Definition of done for a PARITY.md item

An item is `- [x]` only when it is implemented **and** verified against the live **test**
backend (`scripts/live-test-backend.sh`), with an automated test where possible (unit test
in core, plus a check in `tests/live_backend.rs`). The note after `⟶` says how and when it
was verified. Built-but-unverified is `- [~]` with the gap stated.

## 12. Status vocabulary

Use exactly these words when reporting:

* **implemented** — code written, compiles.
* **tested locally** — the named tests actually ran and passed (say which).
* **pushed** — on the remote branch / in a draft PR.
* **merged** — the user merged the PR. Agents never do this.
