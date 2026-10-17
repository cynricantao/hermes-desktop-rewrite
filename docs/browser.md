# The in-app browser

The desktop app's in-app browser is a **separate program**: `helpers/hermes-browser`, a GTK3
window with a [wry](https://crates.io/crates/wry) (WebKitGTK) webview, launched by the app
when a preview is opened and driven over a small line-delimited JSON protocol.

Nothing in `crates/` links WebKit. `crates/hermes-core/src/browser.rs` holds the controller
half of the protocol: command/answer types, helper discovery, the process handle and the
mapping to the backend's `preview.read` / `preview.act` answers.

## Why a separate program

Linking wry + GTK into the Slint app was measured on the throwaway branch
`experiment/webview-wry`: **+46 ms first frame and +20 MiB PSS** for a feature most sessions
never open. The app therefore ships the browser as a second binary (`hermes-browser`,
installed next to `hermes-desktop-rs`) and starts it only on demand. The helper is also
excluded from the workspace (`Cargo.toml` → `[workspace] exclude`), so its dependency tree
(WebKit, GTK, soup…) can never leak into the app's build, `cargo clippy --workspace` or
`cargo test --workspace`.

## Architecture

```
hermes-ui (controller)
   │  server request preview.read / preview.act
   ▼
hermes-core::browser            wire: line-delimited JSON, stdin/stdout
   │  Command / Answer + id matching, 30 s timeout
   ▼
hermes-browser (separate process, GTK main thread)
   │  wry WebViewBuilder::build_gtk into a gtk::Box
   ▼
WebKitGTK (its own web/content processes)
```

* **One window, one page.** No tabs; the pane's multi-tab bookkeeping (`previewTabsFor`,
  hovered/focused zone resolution) has no counterpart here.
* **Threading.** `hermes-browser` runs GTK's main loop on the main thread; a reader thread
  decodes stdin lines and hands them to that loop over a GLib channel. Every webview call —
  `evaluate_script_with_callback`, `load_url`, the top-bar buttons — happens on the GTK
  main thread. JS answers come back on the same thread.
* **Lifecycle.** The app spawns the helper, talks to it, and either sends `shutdown` or drops
  the handle. Dropping closes the helper's stdin; the helper treats stdin EOF as "the app is
  gone" and exits, so a crashed app cannot leave an orphan window. `kill_on_drop` is the
  belt to that braces.

## The protocol

One JSON object per line. Requests go on the helper's stdin; answers and notices come back
on its stdout (stdout carries protocol lines and nothing else — helper logging goes to
stderr, which the app inherits).

| command | fields | answer `result` |
|---|---|---|
| `navigate` | `url` | `{"url": …, "loading": true}` — answered on submission, not on load |
| `back` / `forward` / `reload` | — | `{}` (the controller words the pane's `{acted, note, success}`) |
| `read` | — | `{"url", "title", "text", "total", "truncated"}` — `document.body.innerText`, cut at 200 000 UTF-16 units with the page's own length in `total` |
| `click` | `selector`, or `x` + `y` | `{"tag", "label", "url", "title"}` |
| `type` | `selector`, `text`, `submit?` | `{"tag", "label", "submitted", "url", "title"}` |
| `scroll` | `amount?` (px, negative = up; default one screen) or `to` (`top`/`bottom`) | `{"range", "to", "amount", "url", "title"}` — `range` 0 means nothing to scroll |
| `wait-for-load` | `timeout_ms?` (default 20 000) | `{"ready_state", "url", "title", "timed_out"?}` |
| `shutdown` | — | `{"closed": true}`, then the window closes and the process exits |

Answers echo the request id: `{"id":1,"ok":true,"result":{…}}` or
`{"id":1,"ok":false,"error":"…"}`. Two unsolicited notices carry no id:
`{"event":"ready"}` once the window is up, `{"event":"closed"}` when it goes away. An
unknown verb answers `unsupported command '<verb>' — this helper understands: …`; a line
with no usable id is logged on stderr instead.

The two hand-synced copies of these types are `crates/hermes-core/src/browser.rs` (normative
documentation) and `helpers/hermes-browser/src/protocol.rs`; both files' tests pin the same
lines.

## Wiring in the app (done)

`crates/hermes-ui/src/browser.rs` runs the helper on its own tokio task (like the
Capabilities task), so a slow page never stalls chat. The controller routes:

* the agent's `preview.open` / `preview.close` events → open (or reuse) / close the window
  (`preview_target_url` adds `file://`, `http://localhost…` or `https://` as needed);
* `preview.read` / `preview.act` server requests → the helper, answered as
  `{value: JSON.stringify(result)}` like the Electron pane (`server_request_answer`);
  `preview.act` from a session that is not on screen is refused with the pane's message;
* `/preview <url or path>` and `/preview close` typed in the composer (Hermes's own
  `/browser` command is the CDP connector, so the app uses `/preview`).

If the helper is not installed the user sees a banner saying how to build it.
`packaging/PKGBUILD` builds it and installs it to `/usr/bin/hermes-browser`.

## What the controller does with it

* **`preview.read`** (`ReadRangeRequestParams`): `preview_read_params` → a `read` command →
  `PageText` → `preview_read_answer`, which windows the text exactly like the pane's
  `windowText`: `start` clamped into the text, `count` clamped to `1..=24 000`
  (`PREVIEW_READ_MAX_CHARS`), `total_chars` the text's length in UTF-16 code units (what
  JavaScript counts), and the answer shaped as `PreviewReadResult`
  (`kind`, `title`, `url`, `start`, `end`, `text`, `total_chars`, optional `note`). With no
  page at all the answer is the empty string (`NOTHING_OPEN_VALUE`), as in
  `server-requests.ts`'s `answerValue`; a page that is still booting gets
  `preview_read_not_ready`, the reader's identity answer with its "retry" note.
* **`preview.act`** (`PreviewActRequestParams`, i.e. `drive_preview`'s field set):
  `preview_act_params` → a command, or an `Err` whose message the controller answers as
  `{success:false, error}`. Supported: `back`, `forward`, `reload`, `click`
  (CSS selector), `type` (selector + text + submit), `scroll` (amount or `to`). Refused with
  a clear message: `elements`, `hover`, `press`, `strobe`, `pin`, `unpin`, `hold`, and any
  verb addressed by `ref` instead of a selector. Answers are shaped as `PreviewActResult`
  (`preview_act_answer`): `{success, acted, url?, title?, note?}` — the pane's `acted`
  wording (`clicked button "Sign in"`, `typed into input "Name" and submitted`,
  `scrolled the page`) and its notes (`Page is loading — call elements to see what is on
  it.`, `The page has nothing to scroll — it all fits already.`) are reproduced verbatim.
* **Opening a page** is a helper concern, not a `preview.*` mapping: `BrowserHelper::launch()`
  (discovery: `HERMES_BROWSER_HELPER`, then a `hermes-browser` next to the executable, then
  `PATH`), then `send(Command::Navigate { url })`, then `WaitForLoad`.

A command's answer is matched by id and bounded by `BrowserHelper::timeout`
(`DEFAULT_TIMEOUT` = 30 s, above the helper's own 20 s load deadline). `is_running()` tells
the controller when the window has gone; `send` answers `HelperError::Gone` in that case.

## What is not ported from Electron's preview pane

The pane is a ~9.7 k-line subsystem (preview-act, the in-page engine, the watch overlay,
tabs, console, tour, pop-out bridge, file/artifact previews). The helper implements the
verbs the agent's tools actually reach for and drops the rest:

* **No element inventory.** The engine's refs (`btn-sign-in`), the delta protocol
  (`added`/`changed`/`removed`/`rebound`), the re-bind ladder and `stableOf` live in
  `apps/desktop/src/lib/preview-act/` — none of it is ported. `elements` is refused, and
  answers carry no `elements`/`delta` field.
* **No overlay.** `watch-in-page.ts`, the aim marks, pins, holds and strobing
  (`annotate_preview`) are absent: there is nothing to draw them with.
* **No real input.** WebKitGTK's wry binding exposes no `sendInputEvent`, so clicks and keys
  are dispatched as full synthetic event sequences (pointer/mouse + keyboard, bubbling,
  `view: window`). That is Electron's own fallback for panes with no input channel — "worse
  but still works" — and it means: pages can tell the events apart (`isTrusted === false`),
  the browser's own hover/focus target never moves, and a `type` commits the value through
  the element's native setter + `input`/`change` (Electron's masked-field fallback) rather
  than by synthesising per-character keystrokes.
* **No wheel gestures.** `scroll` uses `window.scrollBy`/`scrollTo`, so scroll-linked
  headers, reveal animations and infinite-scroll loaders stay asleep through it — the same
  caveat Electron records for its scripted path. A bare scroll moves one viewport
  (≈ 0.9 × innerHeight), honouring `prefers-reduced-motion`.
* **No file/artifact previews, console, tours, multi-tab bookkeeping or pop-out.** Those
  stay the Slint app's business, or unbuilt.
* **Truncation.** `read` brings back at most 200 000 UTF-16 units of innerText (the pane
  reads whatever the page holds); the answer's `note` says when it cut. Windowing inside
  that buffer is faithful; a UTF-16 offset that lands inside a surrogate pair rounds down to
  the pair's start, because Rust strings cannot hold half a pair.

## Tests

* `cargo test -p hermes-core browser::` — the wire, discovery order (temp dirs), both
  mappings, and the answer shapes, with fixtures copied from the Electron/backend sources.
  No display, no process.
* `helpers/hermes-browser` → `cargo test` — the wire format, the injected scripts and the
  URL-entry rule.
* `crates/hermes-core/tests/browser_helper.rs` — the end-to-end check: spawns the built
  helper against a local `file://` page, navigates, reads, clicks, types and submits through
  the real mapping functions. **Ignored by default**, because it opens a GTK window and must
  never appear on the user's desktop:

  ```sh
  cd helpers/hermes-browser && CARGO_TARGET_DIR=/tmp/helper-target cargo build --release
  cd ../..
  HERMES_BROWSER_HELPER="$PWD/helpers/hermes-browser/…/hermes-browser" \
    scripts/headless.sh cargo test -p hermes-core --test browser_helper -- --ignored --nocapture
  ```

  `scripts/headless.sh` gives it a private Weston; any private headless display
  (Xvfb/xvfb-run/cage/Weston) does. Without one, do not run it on the user's session.

## Packaging

The Arch package installs `hermes-browser` beside the app binary; discovery finds it there
without configuration. `HERMES_BROWSER_HELPER` overrides the path (used by the tests).
