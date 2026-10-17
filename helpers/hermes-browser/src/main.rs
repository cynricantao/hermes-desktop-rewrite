//! `hermes-browser` — the standalone in-app browser window for Hermes Desktop.
//!
//! A GTK3 window holding a wry (WebKitGTK) webview under a small top bar
//! (Back / Forward / Reload buttons and a URL entry), driven over a
//! line-delimited JSON protocol on stdin/stdout. The controller's half of that
//! protocol — command/event types, discovery, process handle and the mapping to
//! the backend's `preview.read` / `preview.act` answers — lives in
//! `crates/hermes-core/src/browser.rs`, which documents the wire normatively.
//!
//! This program is built and shipped OUTSIDE the Cargo workspace (the root
//! `Cargo.toml` lists `helpers/hermes-browser` in `exclude`) because linking
//! wry + GTK + WebKit into the Slint app itself measured +46 ms first frame and
//! +20 MiB PSS. Nothing here is on the app's startup path.
//!
//! Threading: every webview call happens on the GTK main thread. The stdin
//! reader thread only decodes lines and hands them over a GLib channel; this is
//! the only file that touches wry.
//!
//! Usage: `hermes-browser [url]` — the URL is optional (default `about:blank`).

mod page;
mod protocol;

use std::cell::{Cell, RefCell};
use std::io::{self, BufRead, Write};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gtk::prelude::*;
use protocol::{Command, Request, Signal};
use serde_json::{Value, json};
use wry::{PageLoadEvent, WebView, WebViewBuilder, WebViewBuilderExtUnix};

use crate::protocol::clip;

/// How long a mutating command is given to settle before its answer re-reads
/// the page for a fresh url/title — the Electron engine's `SETTLE_MS`
/// (`apps/desktop/src/app/chat/right-rail/preview-act.ts`).
const SETTLE_MS: u64 = 220;

/// Default `wait-for-load` deadline; the controller can shorten or extend it.
const DEFAULT_LOAD_TIMEOUT_MS: u64 = 20_000;

/// How often `wait-for-load` re-asks the document for its `readyState`.
const LOAD_POLL_MS: u64 = 100;

/// Set when this helper starts a navigation (navigate/back/forward/reload) and
/// cleared when WebKit reports the load finished. Right after `load_url` the old
/// document (often `about:blank`) still answers `readyState === "complete"`, so
/// `wait-for-load` must not trust readiness until the new load has finished.
static NAVIGATION_PENDING: AtomicBool = AtomicBool::new(false);

/// Answer when the window has gone away mid-command (the user closed it while a
/// script was in flight).
const WINDOW_GONE: &str = "the browser window is gone";

/// The window and the pieces commands reach: held for the process lifetime.
struct App {
    window: gtk::Window,
    webview: Rc<WebView>,
    url_entry: gtk::Entry,
}

thread_local! {
    /// The one window. Commands run on the GTK main thread, so this is only ever
    /// touched from there.
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    /// `closed` is emitted exactly once, however the window ends.
    static CLOSED: Cell<bool> = const { Cell::new(false) };
}

fn main() -> std::process::ExitCode {
    if let Err(error) = run() {
        eprintln!("hermes-browser: {error}");
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}

/// Bring the window up, then pump commands and GTK until the window ends.
fn run() -> Result<(), String> {
    let start_url = std::env::args()
        .nth(1)
        .filter(|argument| !argument.starts_with('-'))
        .unwrap_or_else(|| "about:blank".to_owned());

    gtk::init().map_err(|error| format!("gtk init failed: {error}"))?;

    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title("Hermes browser");
    window.set_default_size(1024, 720);
    window.connect_destroy(|_| close_window());

    let column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    window.add(&column);

    // ── Top bar ──
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    bar.set_margin_top(4);
    bar.set_margin_bottom(4);
    bar.set_margin_start(4);
    bar.set_margin_end(4);
    let back = gtk::Button::from_icon_name(Some("go-previous-symbolic"), gtk::IconSize::Button);
    let forward = gtk::Button::from_icon_name(Some("go-next-symbolic"), gtk::IconSize::Button);
    let reload = gtk::Button::from_icon_name(Some("view-refresh-symbolic"), gtk::IconSize::Button);
    back.set_tooltip_text(Some("Back"));
    forward.set_tooltip_text(Some("Forward"));
    reload.set_tooltip_text(Some("Reload"));
    let url_entry = gtk::Entry::new();
    url_entry.set_hexpand(true);
    url_entry.set_placeholder_text(Some("Enter a URL"));
    bar.pack_start(&back, false, false, 0);
    bar.pack_start(&forward, false, false, 0);
    bar.pack_start(&reload, false, false, 0);
    bar.pack_start(&url_entry, true, true, 0);
    column.pack_start(&bar, false, false, 0);

    // ── Webview ──
    let entry_for_load = url_entry.clone();
    let webview = WebViewBuilder::new()
        .with_url(&start_url)
        .with_on_page_load_handler(move |event, url| {
            // Keep the URL bar showing where the page actually went.
            match event {
                PageLoadEvent::Started => NAVIGATION_PENDING.store(true, Ordering::Relaxed),
                PageLoadEvent::Finished => {
                    NAVIGATION_PENDING.store(false, Ordering::Relaxed);
                    entry_for_load.set_text(&url);
                }
            }
        })
        .build_gtk(&column)
        .map_err(|error| format!("could not create the webview: {error}"))?;
    let webview = Rc::new(webview);
    url_entry.set_text(&start_url);

    // ── Top bar wiring (the buttons are the same verbs the protocol sends) ──
    back.connect_clicked({
        let webview = Rc::clone(&webview);
        move |_| {
            let _ = webview.go_back();
        }
    });
    forward.connect_clicked({
        let webview = Rc::clone(&webview);
        move |_| {
            let _ = webview.go_forward();
        }
    });
    reload.connect_clicked({
        let webview = Rc::clone(&webview);
        move |_| {
            let _ = webview.reload();
        }
    });
    url_entry.connect_activate({
        let webview = Rc::clone(&webview);
        move |entry| {
            let _ = webview.load_url(&normalize_url(&entry.text()));
        }
    });

    APP.with(|cell| {
        *cell.borrow_mut() = Some(App {
            window: window.clone(),
            webview,
            url_entry,
        })
    });
    window.show_all();

    // ── Command pump ──
    // The stdin thread decodes lines and hands them here; the receiver is
    // attached to this thread's (default) main context, so on_signal runs on the
    // GTK main thread — the only place a webview may be touched.
    // (`MainContext::channel` is deprecated upstream in favour of an async
    // channel; taking that hint would add a fifth dependency to a program the
    // brief confines to four.)
    #[allow(deprecated)]
    let (sender, receiver) = gtk::glib::MainContext::channel::<Signal>(gtk::glib::Priority::DEFAULT);
    receiver.attach(None, |signal| {
        on_signal(signal);
        gtk::glib::ControlFlow::Continue
    });
    std::thread::Builder::new()
        .name("hermes-browser-stdin".to_owned())
        .spawn(move || read_stdin(sender))
        .map_err(|error| format!("could not start the stdin reader: {error}"))?;

    emit(json!({ "event": "ready" }));
    gtk::main();

    // Leaving the loop: drop the window so WebKit tears its processes down
    // before the process exits.
    APP.with(|cell| {
        if let Some(app) = cell.borrow_mut().take() {
            app.window.close();
        }
    });
    Ok(())
}

// ── The wire ────────────────────────────────────────────────────────────────────

/// Write one protocol event to stdout. The only place anything is printed:
/// stdout carries protocol lines and nothing else.
fn emit(event: Value) {
    let mut out = io::stdout().lock();
    let _ = writeln!(out, "{event}");
    let _ = out.flush();
}

/// Answer a request with a result object.
fn answer(id: u64, result: Value) {
    emit(json!({ "id": id, "ok": true, "result": result }));
}

/// Answer a request with an error message.
fn answer_error(id: u64, message: impl Into<String>) {
    emit(json!({ "id": id, "ok": false, "error": message.into() }));
}

/// Answer-once latch: a script callback that fires twice, or a settle racing a
/// failure, must not put a second line on the wire for one id.
#[derive(Clone)]
struct Once(Arc<AtomicBool>);

impl Once {
    fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    /// True for the first caller only.
    fn claim(&self) -> bool {
        !self.0.swap(true, Ordering::Relaxed)
    }

    /// Whether some caller already answered.
    fn is_done(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

fn answer_once(once: &Once, id: u64, result: Value) {
    if once.claim() {
        answer(id, result);
    }
}

fn answer_error_once(once: &Once, id: u64, message: impl Into<String>) {
    if once.claim() {
        answer_error(id, message);
    }
}

/// Read protocol lines until stdin closes; every line goes to the GTK loop
/// over `sender`. Decoding only — no webview is touched from this thread.
fn read_stdin(sender: gtk::glib::Sender<Signal>) {
    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        if sender.send(protocol::parse_line(&line)).is_err() {
            return;
        }
    }
    // The controller closed the pipe (or died): the window should not outlive
    // the app that asked for it.
    let _ = sender.send(Signal::StdinClosed);
}

/// Handle one decoded stdin line on the GTK main thread.
fn on_signal(signal: Signal) {
    match signal {
        Signal::Request(request) => on_request(*request),
        Signal::Bad { id, message } => match id {
            Some(id) => answer_error(id, message),
            None => eprintln!("hermes-browser: {message}"),
        },
        Signal::StdinClosed => close_window(),
    }
}

/// Run one command. Each arm answers exactly once.
fn on_request(request: Request) {
    let Request { id, command } = request;
    match command {
        Command::Navigate { url } => {
            NAVIGATION_PENDING.store(true, Ordering::Relaxed);
            with_app(id, |app| match app.webview.load_url(&url) {
                Ok(()) => {
                    app.url_entry.set_text(&url);
                    answer(id, json!({ "url": url, "loading": true }));
                }
                Err(error) => answer_error(id, format!("could not load {url}: {error}")),
            })
        }
        Command::Back => {
            NAVIGATION_PENDING.store(true, Ordering::Relaxed);
            act_simple(id, "go back", |app| {
                app.webview.go_back().map_err(|error| error.to_string())
            })
        }
        Command::Forward => {
            NAVIGATION_PENDING.store(true, Ordering::Relaxed);
            act_simple(id, "go forward", |app| {
                app.webview.go_forward().map_err(|error| error.to_string())
            })
        }
        Command::Reload => {
            NAVIGATION_PENDING.store(true, Ordering::Relaxed);
            act_simple(id, "reload", |app| {
                app.webview.reload().map_err(|error| error.to_string())
            })
        }
        Command::Read => eval_and_answer(id, page::read()),
        Command::Click { selector, x, y } => eval_then_settle(id, page::click(selector.as_deref(), x, y)),
        Command::Type { selector, text, submit } => eval_then_settle(id, page::type_text(&selector, &text, submit)),
        Command::Scroll { amount, to } => eval_then_settle(id, page::scroll(amount, to.as_deref())),
        Command::WaitForLoad { timeout_ms } => {
            wait_for_load(id, timeout_ms.unwrap_or(DEFAULT_LOAD_TIMEOUT_MS));
        }
        Command::Shutdown => {
            answer(id, json!({ "closed": true }));
            close_window();
        }
    }
}

/// Run `body` against the live window, or answer that it has gone.
fn with_app(id: u64, body: impl FnOnce(&App)) {
    APP.with(|cell| match cell.borrow().as_ref() {
        Some(app) => body(app),
        None => answer_error(id, WINDOW_GONE),
    });
}

/// A command whose whole effect is on the view itself (history, reload). The
/// answer carries no data: the controller words it exactly like the Electron
/// pane's `{acted, note, success}` answer.
fn act_simple(id: u64, verb: &str, run: impl FnOnce(&App) -> Result<(), String>) {
    with_app(id, |app| match run(app) {
        Ok(()) => answer(id, json!({})),
        Err(error) => answer_error(id, format!("could not {verb}: {error}")),
    });
}

/// Evaluate `js` (which returns an object) and answer with it.
fn eval_and_answer(id: u64, js: String) {
    let once = Once::new();
    with_app(id, |app| {
        let in_callback = once.clone();
        let started = app
            .webview
            .evaluate_script_with_callback(&js, move |raw: String| match page_result(&raw) {
                Ok(value) => answer_once(&in_callback, id, value),
                Err(message) => answer_error_once(&in_callback, id, message),
            });
        if let Err(error) = started {
            answer_error_once(&once, id, format!("the script could not be run: {error}"));
        }
    });
}

/// Run a mutating script, let the page react for [`SETTLE_MS`], then answer with
/// the script's report plus the live url/title — the Electron engine re-reads
/// the page after every action for exactly this reason.
fn eval_then_settle(id: u64, js: String) {
    let once = Once::new();
    with_app(id, |app| {
        let in_callback = once.clone();
        let started = app.webview.evaluate_script_with_callback(&js, move |raw: String| {
            let report = match page_result(&raw) {
                Ok(report) => report,
                Err(message) => return answer_error_once(&in_callback, id, message),
            };
            let after_settle = in_callback.clone();
            gtk::glib::timeout_add_local_once(Duration::from_millis(SETTLE_MS), move || {
                APP.with(|cell| {
                    let borrowed = cell.borrow();
                    let Some(app) = borrowed.as_ref() else {
                        return answer_error_once(&after_settle, id, WINDOW_GONE);
                    };
                    let in_read = after_settle.clone();
                    let started = app
                        .webview
                        .evaluate_script_with_callback(&page::identity(), move |raw: String| {
                            answer_once(&in_read, id, merge_identity(report.clone(), &raw));
                        });
                    if let Err(error) = started {
                        answer_error_once(&after_settle, id, format!("the script could not be run: {error}"));
                    }
                });
            });
        });
        if let Err(error) = started {
            answer_error_once(&once, id, format!("the script could not be run: {error}"));
        }
    });
}

/// Fold the live url/title into a script's report. A page that went away right
/// after the action answers without them rather than failing: the action very
/// likely landed, and it was a navigation that ate the read-back.
fn merge_identity(mut report: Value, raw: &str) -> Value {
    if let Ok(Value::Object(identity)) = serde_json::from_str::<Value>(raw)
        && let Value::Object(base) = &mut report
    {
        for key in ["url", "title"] {
            if let Some(value) = identity.get(key).cloned() {
                base.insert(key.to_owned(), value);
            }
        }
    }
    report
}

/// `wait-for-load`: answer once the document reports `readyState === "complete"`,
/// or — when `timeout_ms` runs out first — with the last state seen.
///
/// Polls on a timer rather than chaining from each script callback: WebKit drops
/// the callback of a script that was running when the document was replaced, so a
/// check issued mid-navigation never answers. Each tick asks again; the first
/// "complete" (or the deadline) answers, and the timer stops once answered.
fn wait_for_load(id: u64, timeout_ms: u64) {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let once = Once::new();
    let last_seen: Arc<std::sync::Mutex<Option<Value>>> = Arc::default();
    poll_ready_state(id, &once, &last_seen);
    gtk::glib::timeout_add_local(Duration::from_millis(LOAD_POLL_MS), move || {
        if once.is_done() {
            return gtk::glib::ControlFlow::Break;
        }
        if Instant::now() >= deadline {
            let mut stuck = last_seen
                .lock()
                .ok()
                .and_then(|seen| seen.clone())
                .unwrap_or_else(|| json!({ "ready_state": "unknown" }));
            if let Value::Object(map) = &mut stuck {
                map.insert("timed_out".to_owned(), Value::Bool(true));
            }
            answer_once(&once, id, stuck);
            return gtk::glib::ControlFlow::Break;
        }
        poll_ready_state(id, &once, &last_seen);
        gtk::glib::ControlFlow::Continue
    });
}

/// One readiness check: answers on "complete", otherwise records what it saw.
fn poll_ready_state(id: u64, once: &Once, last_seen: &Arc<std::sync::Mutex<Option<Value>>>) {
    APP.with(|cell| {
        let borrowed = cell.borrow();
        let Some(app) = borrowed.as_ref() else {
            return answer_error_once(once, id, WINDOW_GONE);
        };
        let in_callback = once.clone();
        let seen = Arc::clone(last_seen);
        // A failed start (page mid-swap) is retried by the next tick.
        let _ = app
            .webview
            .evaluate_script_with_callback(&page::ready_state(), move |raw: String| {
                let Ok(value) = page_result(&raw) else { return };
                let complete = value.get("ready_state").and_then(Value::as_str) == Some("complete");
                if complete && !NAVIGATION_PENDING.load(Ordering::Relaxed) {
                    answer_once(&in_callback, id, value);
                } else if let Ok(mut slot) = seen.lock() {
                    *slot = Some(value);
                }
            });
    });
}

/// The answer object of a script: `Err` for a script-reported `error` field, an
/// empty answer (the script threw, or the document went away) and anything that
/// is not a JSON object.
fn page_result(raw: &str) -> Result<Value, String> {
    if raw.is_empty() {
        return Err("the page did not answer — the script failed or the document has gone".to_owned());
    }
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Object(map)) => match map.get("error").and_then(Value::as_str) {
            Some(error) => Err(error.to_owned()),
            None => Ok(Value::Object(map)),
        },
        Ok(other) => Err(format!(
            "the page answered with something that is not an object: {}",
            clip(&other.to_string(), 200)
        )),
        Err(_) => Err(format!(
            "the page answered with something that is not JSON: {}",
            clip(raw, 200)
        )),
    }
}

/// Emit `closed` once and leave the GTK main loop.
fn close_window() {
    if CLOSED.with(|closed| !closed.replace(true)) {
        emit(json!({ "event": "closed" }));
    }
    gtk::main_quit();
}

/// What the URL entry accepts: anything that already names a scheme is used as
/// written, a bare `host[:port]/path` becomes `https://…`. Not a search box.
fn normalize_url(text: &str) -> String {
    const SCHEMES: [&str; 8] = [
        "http://",
        "https://",
        "file:",
        "about:",
        "data:",
        "blob:",
        "view-source:",
        "mailto:",
    ];
    let trimmed = text.trim();
    if trimmed.is_empty() {
        "about:blank".to_owned()
    } else if SCHEMES.iter().any(|scheme| trimmed.starts_with(scheme)) {
        trimmed.to_owned()
    } else {
        format!("https://{trimmed}")
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_url;

    #[test]
    fn a_bare_host_becomes_https() {
        assert_eq!(normalize_url("example.com/a?b=c"), "https://example.com/a?b=c");
        assert_eq!(normalize_url("  localhost:8080  "), "https://localhost:8080");
    }

    #[test]
    fn anything_with_a_scheme_is_left_alone() {
        for url in [
            "https://example.com/",
            "http://localhost:3000/",
            "file:///tmp/a.html",
            "file:/tmp/a.html",
            "about:blank",
            "data:text/html,<p>hi</p>",
        ] {
            assert_eq!(normalize_url(url), url, "{url}");
        }
    }

    #[test]
    fn an_empty_entry_is_a_blank_page() {
        assert_eq!(normalize_url("   "), "about:blank");
    }
}
