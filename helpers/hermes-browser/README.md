# hermes-browser

The in-app browser for Hermes Desktop: a small GTK3 + [wry](https://crates.io/crates/wry)
(WebKitGTK) window with a top bar (Back / Forward / Reload + URL entry), driven by the app
over line-delimited JSON on stdin/stdout.

It is **not** a workspace member (the root `Cargo.toml` lists it in `exclude`) on purpose:
linking wry + GTK + WebKit into the Slint app measured **+46 ms first frame and +20 MiB
PSS**, so the browser ships as a separate program that the app launches only when the user
opens a preview.

* Protocol, discovery and the mapping to the backend's `preview.read` / `preview.act`
  answers: `crates/hermes-core/src/browser.rs` (the controller half).
* Architecture, what is ported from the Electron preview pane and what is not:
  `docs/browser.md`.

```sh
CARGO_TARGET_DIR=/path/to/target cargo build --release   # → target/release/hermes-browser
./hermes-browser https://example.com/                    # or: hermes-browser file:///tmp/a.html

# drive it by hand:
printf '%s\n' '{"id":1,"cmd":"navigate","url":"https://example.com/"}' \
              '{"id":2,"cmd":"wait-for-load"}' \
              '{"id":3,"cmd":"read"}' | ./hermes-browser
```

`cargo test` here covers the wire format (`protocol.rs`), the injected scripts
(`page.rs`) and the URL-entry rule — no display needed. The end-to-end check that drives a
real page lives in `crates/hermes-core/tests/browser_helper.rs` (ignored; needs a display).
