//! In-app browser: the control protocol for the separate `hermes-browser` helper program
//! (line-delimited JSON over stdin/stdout), helper discovery and process handle, and the
//! mapping between the backend's `preview.read` / `preview.act` server requests and helper
//! commands + answers. UI-free.
//!
//! # Why a helper program
//!
//! The browser is a separate binary (`helpers/hermes-browser`, GTK + wry/WebKitGTK, outside
//! the workspace) that the app launches when a preview is opened. Linking wry + GTK into
//! the Slint app itself measured **+46 ms first frame and +20 MiB PSS**, so nothing in this
//! module links WebKit — it only talks to the process. Discovery order is
//! `HERMES_BROWSER_HELPER`, then a `hermes-browser` file next to the running executable
//! (how packaging installs it), then `PATH`.
//!
//! # The wire
//!
//! One JSON object per line, in both directions, on the helper's stdin/stdout:
//!
//! * request — `{"id":1,"cmd":"navigate","url":"https://example.com/"}`
//!   (`id` is ours; every command type has its own `cmd` tag, see [`Command`])
//! * answer — `{"id":1,"ok":true,"result":{…}}` or `{"id":1,"ok":false,"error":"…"}`
//! * notice (no id) — `{"event":"ready"}` once the window is up, `{"event":"closed"}` when
//!   it goes away (the user closed it, or stdin reached EOF).
//!
//! The helper's own copy of these types is `helpers/hermes-browser/src/protocol.rs`; the
//! two are kept in sync by hand (the helper cannot depend on this crate) and both files'
//! tests pin the same lines.
//!
//! # The controller's use of it
//!
//! ```text
//! preview.read  → PreviewReadParams → preview_read_params() → Command::Read
//!               → BrowserHelper::send() → Answer → PageText → preview_read_answer()
//! preview.act   → PreviewActParams  → preview_act_params()  → Command (+ Err message)
//!               → BrowserHelper::send() → Answer → preview_act_answer() / preview_act_error()
//! ```
//!
//! The answer is the value the controller replies with to the backend's server request:
//! server-requests.ts answers with `{value: JSON.stringify(result)}` (`answerValue`), the
//! empty string meaning "nothing open", and the JSON is exactly the shape this module
//! produces. The two `preview.*` request schemas are
//! `PreviewActRequestParams` / `ReadRangeRequestParams` in
//! `apps/shared/src/gateway-contract.openrpc.json`; the answers are
//! `PreviewActResult` (`apps/desktop/src/lib/preview-act/types.ts`) and
//! `PreviewReadResult` (`preview-reader.ts`).
//!
//! # What is not ported
//!
//! Electron's preview pane is a ~9.7k-line subsystem. The helper implements the verbs the
//! agent's `drive_preview` tool sends — back / forward / reload / click / type / scroll —
//! and the `read` behind `desktop_preview`'s read; `elements`, `hover`, `press`, `strobe`
//! and the annotation verbs (`pin` / `unpin` / `hold`) answer a clear unsupported-action
//! error, because the ref-minting inventory engine and the watch overlay are not ported.
//! See `docs/browser.md`.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command as ProcessCommand};
use tokio::sync::{Mutex, oneshot};

// ── Protocol: commands ──────────────────────────────────────────────────────────

/// One command for the helper; `cmd` tags the verb on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "kebab-case")]
pub enum Command {
    /// Load a URL — any scheme WebKit resolves, including `file://`.
    Navigate { url: String },
    /// Go back one entry in the view's history.
    Back,
    /// Go forward one entry in the view's history.
    Forward,
    /// Reload the current document.
    Reload,
    /// Read the page: URL, title, `document.body.innerText`.
    Read,
    /// Click the element matching `selector`, or the element at (x, y).
    Click {
        #[serde(default)]
        selector: Option<String>,
        #[serde(default)]
        x: Option<f64>,
        #[serde(default)]
        y: Option<f64>,
    },
    /// Put `text` into the field matching `selector`; `submit` also presses Enter and
    /// submits the owning form.
    Type {
        selector: String,
        text: String,
        #[serde(default)]
        submit: bool,
    },
    /// Scroll by `amount` pixels (negative = up; absent = one screen), or to `to`.
    Scroll {
        #[serde(default)]
        amount: Option<f64>,
        #[serde(default)]
        to: Option<ScrollTo>,
    },
    /// Answer once the document reports `readyState === "complete"`, or when `timeout_ms`
    /// runs out (the helper defaults to 20 s).
    WaitForLoad {
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    /// Answer, then close the window and exit.
    Shutdown,
}

impl Command {
    /// The verb's name on the wire — what `{"cmd": …}` carries.
    pub fn name(&self) -> &'static str {
        match self {
            Command::Navigate { .. } => "navigate",
            Command::Back => "back",
            Command::Forward => "forward",
            Command::Reload => "reload",
            Command::Read => "read",
            Command::Click { .. } => "click",
            Command::Type { .. } => "type",
            Command::Scroll { .. } => "scroll",
            Command::WaitForLoad { .. } => "wait-for-load",
            Command::Shutdown => "shutdown",
        }
    }
}

/// Where a `scroll` should land (`drive_preview`'s `to`: `top` / `bottom`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScrollTo {
    Top,
    Bottom,
}

/// One request line: a command plus the id its answer must echo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    #[serde(flatten)]
    pub command: Command,
}

impl Request {
    /// A new request; the id is the controller's to choose (it counts up).
    pub fn new(id: u64, command: Command) -> Self {
        Self { id, command }
    }
}

/// One answer to one [`Request`]. `ok` decides which of `result` / `error` is meaningful.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    pub id: u64,
    pub ok: bool,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<String>,
}

impl Answer {
    /// The result object, or the error message the helper answered with.
    pub fn outcome(&self) -> Result<&Value, &str> {
        if self.ok {
            self.result.as_ref().ok_or("the browser helper answered with no result")
        } else {
            Err(self
                .error
                .as_deref()
                .unwrap_or("the browser helper answered with no error message"))
        }
    }
}

// ── Protocol: encoding / decoding ───────────────────────────────────────────────

/// Serialize a request as one terminated line — what goes on the helper's stdin.
pub fn encode_request(request: &Request) -> String {
    // Cannot fail for these types: strings, integers and finite-or-null floats.
    let mut line = serde_json::to_string(request).expect("a Request always serializes");
    line.push('\n');
    line
}

/// What one line from the helper's stdout is.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// An answer to a request, matched by id.
    Answer(Answer),
    /// The window is up (sent once, before the first answer).
    Ready,
    /// The window went away; the helper is exiting.
    Closed,
    /// A notice this build does not know. Logged, never fatal.
    Unknown(Value),
}

/// Decode one stdout line. `Err` is a malformed line — logged and skipped by the reader
/// loop, never fatal.
pub fn decode_event(line: &str) -> Result<Event, String> {
    let trimmed = line.trim();
    let value: Value =
        serde_json::from_str(trimmed).map_err(|error| format!("line is not JSON ({error}): {}", clip(trimmed, 120)))?;
    let Some(object) = value.as_object() else {
        return Ok(Event::Unknown(value));
    };
    if object.contains_key("id") {
        return serde_json::from_value::<Answer>(value.clone())
            .map(Event::Answer)
            .map_err(|error| format!("answer line is malformed: {error}"));
    }
    match object.get("event").and_then(Value::as_str) {
        Some("ready") => Ok(Event::Ready),
        Some("closed") => Ok(Event::Closed),
        _ => Ok(Event::Unknown(value)),
    }
}

/// First `max` characters of `text`, with an ellipsis when it was cut.
fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let cut: String = text.chars().take(max).collect();
    format!("{cut}…")
}

// ── Discovery ───────────────────────────────────────────────────────────────────

/// The environment variable naming the helper program outright.
pub const HELPER_ENV: &str = "HERMES_BROWSER_HELPER";

/// The helper's file name on disk.
pub const HELPER_NAME: &str = "hermes-browser";

/// Find the helper program: `HERMES_BROWSER_HELPER` first, then a `hermes-browser` file
/// next to the running executable (how packaging installs it), then `PATH`.
pub fn find_helper() -> Option<PathBuf> {
    discover_from(
        std::env::var_os(HELPER_ENV),
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf)),
        std::env::var_os("PATH"),
    )
}

/// [`find_helper`] with the ambient state passed in, so the order can be tested without
/// touching the process environment.
fn discover_from(env: Option<OsString>, exe_dir: Option<PathBuf>, path: Option<OsString>) -> Option<PathBuf> {
    let names = file_names();
    if let Some(variable) = env.filter(|value| !value.is_empty()) {
        let candidate = PathBuf::from(variable);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    if let Some(directory) = exe_dir {
        for name in &names {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    for directory in std::env::split_paths(&path?) {
        for name in &names {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// The file names to look for, in order (`.exe` on Windows).
fn file_names() -> Vec<String> {
    let mut names = vec![HELPER_NAME.to_owned()];
    if cfg!(windows) {
        names.push(format!("{HELPER_NAME}.exe"));
    }
    names
}

// ── Process handle ──────────────────────────────────────────────────────────────

/// How long a command is given to answer before [`BrowserHelper::send`] gives up.
/// Comfortably above the helper's own `wait-for-load` default so a page load that the
/// helper answers itself is never cut off here first.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// How long [`BrowserHelper::shutdown`] waits for the helper to exit after answering.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Something went wrong talking to the helper.
#[derive(Debug, thiserror::Error)]
pub enum HelperError {
    /// No helper program was found on this machine.
    #[error("the browser helper program was not found — build helpers/hermes-browser, or point {HELPER_ENV} at it")]
    NotFound,
    /// The helper program exists but could not be started.
    #[error("could not start the browser helper: {0}")]
    Spawn(String),
    /// The helper exited (or closed its window) before or while answering.
    #[error("the browser helper is not running")]
    Gone,
    /// No answer within [`BrowserHelper::timeout`].
    #[error("the browser helper did not answer within {} ms", .0.as_millis())]
    Timeout(Duration),
    /// Writing to the helper failed.
    #[error("browser helper i/o failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Answers waiting to be matched, by request id.
type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Answer>>>>;

/// A running `hermes-browser` process, spoken to one request at a time.
///
/// The helper owns exactly one window and one page, and the controller answers one
/// `preview.*` server request at a time, so commands need no intra-helper queueing; each
/// answer is matched to its request by id regardless.
///
/// Lifecycle: dropping the handle closes the helper's stdin, and the helper exits when its
/// stdin closes; `kill_on_drop` is the belt to that braces (a crashed app cannot leave an
/// orphan window behind).
pub struct BrowserHelper {
    child: Child,
    stdin: ChildStdin,
    pending: Pending,
    alive: Arc<AtomicBool>,
    next_id: u64,
    timeout: Duration,
}

impl BrowserHelper {
    /// Start the helper at `program` (an absolute path, e.g. from [`find_helper`]).
    ///
    /// Its stderr is inherited, so anything it logs lands in the app's log.
    pub async fn spawn(program: &Path) -> Result<Self, HelperError> {
        let mut child = ProcessCommand::new(program)
            .kill_on_drop(true)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .map_err(|error| HelperError::Spawn(format!("{}: {error}", program.display())))?;
        let stdin = child.stdin.take().ok_or(HelperError::Gone)?;
        let stdout = child.stdout.take().ok_or(HelperError::Gone)?;

        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let alive = Arc::new(AtomicBool::new(true));
        tokio::spawn(read_events(stdout, Arc::clone(&pending), Arc::clone(&alive)));

        Ok(Self {
            child,
            stdin,
            pending,
            alive,
            next_id: 1,
            timeout: DEFAULT_TIMEOUT,
        })
    }

    /// Find the helper ([`find_helper`]) and start it, or report that it is not installed.
    pub async fn launch() -> Result<Self, HelperError> {
        let program = find_helper().ok_or(HelperError::NotFound)?;
        Self::spawn(&program).await
    }

    /// Send one command and wait for its answer (matched by id, bounded by
    /// [`BrowserHelper::timeout`]).
    pub async fn send(&mut self, command: Command) -> Result<Answer, HelperError> {
        if !self.is_running() {
            return Err(HelperError::Gone);
        }
        let id = self.next_id;
        self.next_id += 1;
        let line = encode_request(&Request::new(id, command));
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.flush().await?;

        let (sender, receiver) = oneshot::channel();
        self.pending.lock().await.insert(id, sender);
        match tokio::time::timeout(self.timeout, receiver).await {
            Ok(Ok(answer)) => Ok(answer),
            // The reader task dropped the sender: the helper is gone.
            Ok(Err(_)) => Err(HelperError::Gone),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(HelperError::Timeout(self.timeout))
            }
        }
    }

    /// Whether the helper is still alive — window open, stdout open. Flips to `false`
    /// when it exits, closes its window, or is killed.
    pub fn is_running(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    /// Replace the per-command answer deadline (default [`DEFAULT_TIMEOUT`]).
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// The helper's process id (for logs and for killing it out of band).
    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// Ask the helper to close its window and exit, and give it a moment to go.
    /// Dropping the handle skips the goodbye (stdin closes; the helper follows).
    pub async fn shutdown(mut self) {
        let _ = self.send(Command::Shutdown).await;
        let _ = tokio::time::timeout(SHUTDOWN_GRACE, self.child.wait()).await;
        self.alive.store(false, Ordering::Relaxed);
    }

    /// Signal the helper now: the window disappears and the process follows.
    pub async fn kill(&mut self) -> Result<(), HelperError> {
        self.alive.store(false, Ordering::Relaxed);
        self.child.kill().await?;
        Ok(())
    }
}

/// Read the helper's stdout until it closes: answers to whoever waits on their id, notices
/// to the state they name.
async fn read_events(stdout: ChildStdout, pending: Pending, alive: Arc<AtomicBool>) {
    let mut lines = BufReader::new(stdout).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => match decode_event(&line) {
                Ok(Event::Answer(answer)) => match pending.lock().await.remove(&answer.id) {
                    Some(sender) => {
                        let _ = sender.send(answer);
                    }
                    None => tracing::debug!(id = answer.id, "browser helper answered an id nobody waits for"),
                },
                Ok(Event::Ready) => tracing::debug!("browser helper ready"),
                Ok(Event::Closed) => break,
                Ok(Event::Unknown(notice)) => tracing::debug!(notice = %notice, "browser helper notice"),
                Err(message) => tracing::warn!("browser helper: {message}"),
            },
            Ok(None) => break,
            Err(error) => {
                tracing::warn!("browser helper stdout: {error}");
                break;
            }
        }
    }
    alive.store(false, Ordering::Relaxed);
    // Unblock every waiter: dropping the senders resolves their receivers with `Gone`.
    pending.lock().await.clear();
}

// ── preview.read ────────────────────────────────────────────────────────────────

/// The per-read cap — and the default — on returned characters. preview-reader.ts's
/// `PREVIEW_READ_MAX_CHARS`: a page's innerText crosses the gateway into model context and
/// can be megabytes, so reads page with `start`/`count`.
pub const PREVIEW_READ_MAX_CHARS: usize = 24_000;

/// What `preview.read` answers when there is no preview at all: the empty `value` string.
/// server-requests.ts answers `{value: result ? JSON.stringify(result) : ''}`.
pub const NOTHING_OPEN_VALUE: &str = "";

/// The note `preview.read` carries for a tab whose document is not up yet —
/// preview-reader.ts's identity answer, verbatim.
pub const PAGE_NOT_READY_NOTE: &str = "The page has not finished loading — retry in a moment.";

/// The backend's `preview.read` params (gateway contract `ReadRangeRequestParams`:
/// `session_id` required, `start` / `count` optional).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PreviewReadParams {
    #[serde(default)]
    pub session_id: String,
    /// 0-indexed character offset into the page text (UTF-16 code units, as JS counts).
    #[serde(default)]
    pub start: Option<usize>,
    /// Characters to return, capped at [`PREVIEW_READ_MAX_CHARS`].
    #[serde(default)]
    pub count: Option<usize>,
}

/// What the helper's `read` answers, and what [`preview_read_answer`] windows.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct PageText {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub text: String,
    /// The page's own text length, when the helper had to cut it short.
    #[serde(default)]
    pub total: Option<usize>,
    /// True when the helper's cap cut the text short.
    #[serde(default)]
    pub truncated: bool,
}

impl PageText {
    /// Leniently read a `read` result (a helper from another version answers what it has).
    pub fn from_result(result: &Value) -> Self {
        serde_json::from_value(result.clone()).unwrap_or_default()
    }
}

/// Map `preview.read` params to the helper command that fetches the page.
///
/// `start` / `count` do not go over the wire: the helper answers with the page's text and
/// [`preview_read_answer`] windows it here, so paging a long page costs no second round
/// trip.
pub fn preview_read_params(_params: &PreviewReadParams) -> Command {
    Command::Read
}

/// The exact JSON `preview.read` answers with — `PreviewReadResult`, windowed the way
/// preview-reader.ts's `windowText` windows it: `start` clamped into the text, `count`
/// clamped to `1..=PREVIEW_READ_MAX_CHARS` (default the max), `total_chars` the whole
/// text's length.
pub fn preview_read_answer(params: &PreviewReadParams, page: &PageText) -> Value {
    let total = utf16_len(&page.text);
    let from = params.start.unwrap_or(0).min(total);
    let want = params
        .count
        .unwrap_or(PREVIEW_READ_MAX_CHARS)
        .clamp(1, PREVIEW_READ_MAX_CHARS);
    let to = from.saturating_add(want).min(total);
    let mut answer = json!({
        "kind": "url",
        // target.label is the tab's name; a browser tab is named by its URL when the
        // document has no title yet.
        "title": if page.title.is_empty() { page.url.clone() } else { page.title.clone() },
        "url": page.url.clone(),
        "start": from,
        "end": to,
        "text": utf16_slice(&page.text, from, to),
        "total_chars": total,
    });
    if page.truncated {
        let note = match page.total {
            Some(reported) => {
                format!("The page text was truncated to {total} of {reported} characters by the browser helper.")
            }
            None => format!("The page text was truncated to {total} characters by the browser helper."),
        };
        answer["note"] = Value::String(note);
    }
    answer
}

/// The `preview.read` answer for a page that exists but cannot be read yet — the reader's
/// identity answer: the URL, no text, and the note telling the agent to retry.
pub fn preview_read_not_ready(params: &PreviewReadParams, url: &str) -> Value {
    let page = PageText {
        url: url.to_owned(),
        ..PageText::default()
    };
    let mut answer = preview_read_answer(params, &page);
    answer["note"] = Value::String(PAGE_NOT_READY_NOTE.to_owned());
    answer
}

/// Length in UTF-16 code units — what JavaScript's `String.length` counts, and therefore
/// what the pane's `start` / `end` / `total_chars` mean.
fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// The substring at UTF-16 `from .. to`.
fn utf16_slice(text: &str, from: usize, to: usize) -> &str {
    let start = byte_of_unit(text, from);
    let end = byte_of_unit(text, to).max(start);
    &text[start..end]
}

/// Byte offset of the `n`-th UTF-16 code unit, rounded DOWN to a character boundary — the
/// only rounding Rust can represent (JS would split a surrogate pair; we keep the pair).
fn byte_of_unit(text: &str, unit: usize) -> usize {
    let mut seen = 0;
    for (offset, character) in text.char_indices() {
        if seen >= unit {
            return offset;
        }
        seen += character.len_utf16();
    }
    text.len()
}

// ── preview.open / answers ──────────────────────────────────────────────────────

/// The URL to load for a `preview.open` target. The backend's `open_preview` tool sends a
/// web URL, a `localhost:<port>` dev server, or an absolute file path
/// (`tools/open_preview_tool.py`); WebKit needs a scheme for the last two.
pub fn preview_target_url(target: &str) -> String {
    let target = target.trim();
    if target.contains("://") || target.starts_with("about:") || target.starts_with("data:") {
        return target.to_owned();
    }
    if target.starts_with('/') {
        return format!(
            "file://{}",
            target.replace('%', "%25").replace(' ', "%20").replace('#', "%23")
        );
    }
    if target.starts_with("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return preview_target_url(&format!("{}/{}", home.to_string_lossy(), &target[2..]));
    }
    let host = target.split(['/', ':']).next().unwrap_or_default();
    let local = host == "localhost" || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback());
    format!("{}://{target}", if local { "http" } else { "https" })
}

/// The `result` a `preview.read` / `preview.act` server request is answered with.
/// server-requests.ts `answerValue`: `{value: JSON.stringify(result)}`, or `{value: ""}` when
/// there is nothing to say (no page open).
pub fn server_request_answer(result: Option<&Value>) -> Value {
    match result {
        Some(v) => json!({ "value": v.to_string() }),
        None => json!({ "value": NOTHING_OPEN_VALUE }),
    }
}

/// `preview.act` refusal for a request from a session the user is not looking at —
/// server-requests.ts `previewAct`, verbatim.
pub const NOT_ACTIVE_SESSION_ERROR: &str =
    "The in-app browser only takes actions in the session the user is looking at.";

// ── preview.act ─────────────────────────────────────────────────────────────────

/// The error `preview.act` answers with when no page is open — preview-act.ts's
/// `NOTHING_OPEN`, verbatim.
pub const NOTHING_OPEN_ERROR: &str = "No live page is open in the in-app browser — open one with open_preview first.";

/// The note a history action answers with — preview-act.ts's `NAVIGATED` wording for
/// back / forward / reload, verbatim.
const HISTORY_NOTE: &str = "Page is loading — call elements to see what is on it.";

/// The note a scroll answers with when the page has nothing to scroll — the pane's
/// `driveScroll`, verbatim.
const NOTHING_TO_SCROLL: &str = "The page has nothing to scroll — it all fits already.";

/// The backend's `preview.act` params (gateway contract `PreviewActRequestParams` — the
/// `drive_preview` / `annotate_preview` field sets).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PreviewActParams {
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub action: String,
    /// Element ref from a previous `elements` call — the helper has no refs, so a ref is
    /// refused with a message saying to pass a selector.
    #[serde(default, rename = "ref")]
    pub element_ref: Option<String>,
    #[serde(default)]
    pub selector: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub submit: Option<bool>,
    #[serde(default)]
    pub full: Option<bool>,
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub amount: Option<i64>,
    #[serde(default)]
    pub max: Option<i64>,
    #[serde(default)]
    pub allow_shortcut: Option<bool>,
}

/// Map `preview.act` params to a helper command.
///
/// `Err` is the message the controller answers with (`{success: false, error}`): either the
/// verb is one the helper does not implement, or its target is missing.
pub fn preview_act_params(params: &PreviewActParams) -> Result<Command, String> {
    match params.action.as_str() {
        "back" => Ok(Command::Back),
        "forward" => Ok(Command::Forward),
        "reload" => Ok(Command::Reload),
        "click" => click_command(params),
        "type" => type_command(params),
        "scroll" => scroll_command(params),
        other => Err(unsupported_action(other)),
    }
}

/// `click`: a CSS selector. `drive_preview` also sends refs, which only the unported
/// inventory can mint.
fn click_command(params: &PreviewActParams) -> Result<Command, String> {
    if let Some(selector) = selector_of(params) {
        return Ok(Command::Click {
            selector: Some(selector.to_owned()),
            x: None,
            y: None,
        });
    }
    if let Some(element_ref) = non_empty(params.element_ref.as_deref()) {
        return Err(ref_needs_selector("click", element_ref));
    }
    Err("click needs a CSS selector — the browser helper has no element refs; pass a selector.".to_owned())
}

/// `type`: a CSS selector plus the text. `submit` presses Enter afterwards.
fn type_command(params: &PreviewActParams) -> Result<Command, String> {
    let Some(selector) = selector_of(params) else {
        return Err(match non_empty(params.element_ref.as_deref()) {
            Some(element_ref) => ref_needs_selector("type", element_ref),
            None => "type needs a CSS selector and the text to enter — the browser helper types into a field matched by selector.".to_owned(),
        });
    };
    Ok(Command::Type {
        selector: selector.to_owned(),
        text: params.text.clone().unwrap_or_default(),
        submit: params.submit.unwrap_or(false),
    })
}

/// `scroll`: `to` jumps to an end, `amount` scrolls by pixels, neither scrolls one screen.
fn scroll_command(params: &PreviewActParams) -> Result<Command, String> {
    if let Some(to) = non_empty(params.to.as_deref()) {
        let target = match to {
            "top" => ScrollTo::Top,
            "bottom" => ScrollTo::Bottom,
            other => return Err(format!("to must be one of: top, bottom (got '{other}').")),
        };
        return Ok(Command::Scroll {
            amount: None,
            to: Some(target),
        });
    }
    Ok(Command::Scroll {
        amount: params.amount.map(|amount| amount as f64),
        to: None,
    })
}

/// The message for a verb the helper does not implement: the pane's engine covers it, the
/// helper does not (see the module docs and docs/browser.md).
fn unsupported_action(action: &str) -> String {
    format!(
        "action '{action}' is not supported by the browser helper — it implements back, forward, \
         reload, click, type and scroll; the page-inventory engine the pane runs (elements, \
         hover, press, strobe, annotations) is not ported."
    )
}

/// The message for a verb addressed by a ref, which the helper cannot resolve.
fn ref_needs_selector(verb: &str, element_ref: &str) -> String {
    format!(
        "{verb} by ref ('{element_ref}') is not supported by the browser helper — it has no element \
         inventory, so refs cannot be minted or resolved; pass a CSS selector instead."
    )
}

/// The params' CSS selector, when they carry one.
fn selector_of(params: &PreviewActParams) -> Option<&str> {
    non_empty(params.selector.as_deref())
}

/// `Some(text)` for a non-blank string.
fn non_empty(text: Option<&str>) -> Option<&str> {
    text.map(str::trim).filter(|text| !text.is_empty())
}

/// The exact JSON `preview.act` answers with when the action could not be run —
/// server-requests.ts answers `{error, success: false}` for every failure.
pub fn preview_act_error(message: &str) -> Value {
    json!({ "error": message, "success": false })
}

/// The exact JSON `preview.act` answers with, for the `command` that was sent and the
/// helper's `answer` to it.
///
/// Shape per `PreviewActResult`: `{success, acted?, url?, title?, note?}`. The helper has
/// no page inventory, so `elements` / `delta` are absent — the pane sends them on every
/// mutating action, but nothing downstream requires them (the tool passes the JSON through
/// to the model).
pub fn preview_act_answer(command: &Command, answer: &Answer) -> Value {
    let Ok(result) = answer.outcome() else {
        return preview_act_error(
            answer
                .error
                .as_deref()
                .unwrap_or("the browser helper did not say why it failed"),
        );
    };
    match command {
        Command::Back | Command::Forward | Command::Reload => json!({
            "acted": command.name(),
            "note": HISTORY_NOTE,
            "success": true,
        }),
        Command::Click { .. } | Command::Type { .. } | Command::Scroll { .. } => {
            let mut out = json!({
                "acted": acted_for(command, result),
                "success": true,
            });
            for key in ["url", "title"] {
                if let Some(value) = result.get(key) {
                    out[key] = value.clone();
                }
            }
            if command.name() == "scroll" && result.get("range").and_then(Value::as_u64) == Some(0) {
                out["note"] = Value::String(NOTHING_TO_SCROLL.to_owned());
            }
            out
        }
        // Not a `preview.act` verb (navigate / read / wait-for-load): nothing to say
        // beyond the helper having answered.
        _ => {
            let mut out = json!({ "success": true });
            for key in ["url", "title"] {
                if let Some(value) = result.get(key) {
                    out[key] = value.clone();
                }
            }
            out
        }
    }
}

/// `clicked button "Sign in"` — the engine's `describe()` wording (act-in-page.ts:
/// `tagName.toLowerCase() + ' "' + label + '"'`), from what the helper's script matched.
fn acted_for(command: &Command, result: &Value) -> String {
    let tag = result.get("tag").and_then(Value::as_str).unwrap_or("");
    let label = result.get("label").and_then(Value::as_str).unwrap_or("");
    let target = if tag.is_empty() {
        "the element".to_owned()
    } else if label.is_empty() {
        tag.to_owned()
    } else {
        format!("{tag} \"{label}\"")
    };
    match command {
        // act-in-page.ts: 'typed into ' + describe(el) + (submit ? ' and submitted' : '')
        Command::Type { submit, .. } => format!("typed into {target}{}", if *submit { " and submitted" } else { "" }),
        // act-in-page.ts: driveScroll answers 'scrolled the page'.
        Command::Scroll { .. } => "scrolled the page".to_owned(),
        _ => format!("clicked {target}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── Wire ────────────────────────────────────────────────────────────────────

    #[test]
    fn encodes_a_command_as_one_terminated_line() {
        let line = encode_request(&Request::new(
            7,
            Command::Click {
                selector: Some("#go".to_owned()),
                x: None,
                y: None,
            },
        ));
        assert_eq!(
            line,
            "{\"id\":7,\"cmd\":\"click\",\"selector\":\"#go\",\"x\":null,\"y\":null}\n"
        );
        assert!(line.ends_with('\n'));
    }

    /// The lines the helper actually writes (its README shows the same ones).
    #[test]
    fn decodes_answers_and_notices() {
        assert_eq!(
            decode_event(r#"{"id":3,"ok":true,"result":{"range":0}}"#),
            Ok(Event::Answer(Answer {
                id: 3,
                ok: true,
                result: Some(json!({ "range": 0 })),
                error: None,
            }))
        );
        assert_eq!(
            decode_event(r#"{"id":4,"ok":false,"error":"no element matches #go"}"#),
            Ok(Event::Answer(Answer {
                id: 4,
                ok: false,
                result: None,
                error: Some("no element matches #go".to_owned()),
            }))
        );
        assert_eq!(decode_event(r#"{"event":"ready"}"#), Ok(Event::Ready));
        assert_eq!(decode_event(r#"{"event":"closed"}"#), Ok(Event::Closed));
        assert_eq!(
            decode_event(r#"{"event":"something-new"}"#),
            Ok(Event::Unknown(json!({ "event": "something-new" })))
        );
        assert!(decode_event("not json").is_err());
        assert!(decode_event(r#"{"id":1,"ok":"yes"}"#).is_err());
    }

    #[test]
    fn outcome_reads_the_failure_message() {
        let failed = Answer {
            id: 1,
            ok: false,
            result: None,
            error: Some("the page did not answer".to_owned()),
        };
        assert_eq!(failed.outcome(), Err("the page did not answer"));
        let ok = Answer {
            id: 2,
            ok: true,
            result: Some(json!({ "url": "file:///a.html" })),
            error: None,
        };
        assert_eq!(ok.outcome(), Ok(&json!({ "url": "file:///a.html" })));
    }

    // ── Discovery ───────────────────────────────────────────────────────────────

    /// A throwaway directory; removed on drop. No tempfile dependency, no process env.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!("hermes-core-browser-{}-{tag}", std::process::id()));
            std::fs::create_dir_all(&path).expect("temp dir");
            Self(path)
        }

        fn file(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, b"#!/bin/sh\n").expect("temp file");
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn discovery_prefers_the_env_override() {
        let override_dir = TempDir::new("env");
        let exe_dir = TempDir::new("env-exe");
        let path_dir = TempDir::new("env-path");
        let wanted = override_dir.file(HELPER_NAME);
        exe_dir.file(HELPER_NAME);
        path_dir.file(HELPER_NAME);

        let found = discover_from(
            Some(wanted.clone().into_os_string()),
            Some(exe_dir.0.clone()),
            Some(path_dir.0.clone().into_os_string()),
        );
        assert_eq!(found.as_deref(), Some(wanted.as_path()));
    }

    #[test]
    fn discovery_falls_back_to_the_executable_directory_then_path() {
        let exe_dir = TempDir::new("exe");
        let path_dir = TempDir::new("path");
        let beside = exe_dir.file(HELPER_NAME);
        let on_path = path_dir.file(HELPER_NAME);

        let found = discover_from(None, Some(exe_dir.0.clone()), Some(path_dir.0.clone().into_os_string()));
        assert_eq!(found.as_deref(), Some(beside.as_path()), "exe dir wins over PATH");

        // An executable directory with no helper falls through to PATH.
        let empty = TempDir::new("empty");
        let found = discover_from(None, Some(empty.0.clone()), Some(path_dir.0.clone().into_os_string()));
        assert_eq!(found.as_deref(), Some(on_path.as_path()));
    }

    #[test]
    fn discovery_reports_nothing_when_nothing_is_installed() {
        let empty = TempDir::new("none");
        let missing = empty.0.join("absent-hermes-browser");
        assert_eq!(
            discover_from(
                Some(missing.into_os_string()),
                Some(empty.0.clone()),
                Some(empty.0.clone().into_os_string())
            ),
            None
        );
        assert_eq!(discover_from(None, None, None), None);
    }

    // ── preview.read ────────────────────────────────────────────────────────────

    /// The params as the backend sends them (`ReadRangeRequestParams`), and the answer as
    /// preview-reader.ts's `windowText` computes it.
    #[test]
    fn read_params_map_to_the_read_command() {
        let params: PreviewReadParams =
            serde_json::from_value(json!({ "session_id": "s-1", "start": 0, "count": 5 })).expect("params");
        assert_eq!(params.session_id, "s-1");
        assert_eq!(preview_read_params(&params), Command::Read);
        assert_eq!(
            encode_request(&Request::new(1, preview_read_params(&params))),
            "{\"id\":1,\"cmd\":\"read\"}\n"
        );
    }

    #[test]
    fn read_answer_windows_a_page_like_the_pane() {
        let page = PageText {
            url: "file:///tmp/a.html".to_owned(),
            title: "A page".to_owned(),
            text: "0123456789".to_owned(),
            ..PageText::default()
        };
        let params = |start, count| PreviewReadParams {
            session_id: "s-1".to_owned(),
            start,
            count,
        };

        assert_eq!(
            preview_read_answer(&params(None, None), &page),
            json!({
                "kind": "url",
                "title": "A page",
                "url": "file:///tmp/a.html",
                "start": 0,
                "end": 10,
                "text": "0123456789",
                "total_chars": 10,
            })
        );
        let middle = preview_read_answer(&params(Some(2), Some(3)), &page);
        assert_eq!(middle["start"], json!(2));
        assert_eq!(middle["end"], json!(5));
        assert_eq!(middle["text"], json!("234"));

        // start clamps into the text; count 0 still returns one character (windowText's
        // Math.max(1, count)); a count past the cap is capped.
        let past = preview_read_answer(&params(Some(99), None), &page);
        assert_eq!(past["start"], json!(10));
        assert_eq!(past["end"], json!(10));
        assert_eq!(past["text"], json!(""));
        let zero = preview_read_answer(&params(Some(0), Some(0)), &page);
        assert_eq!(zero["end"], json!(1));
        assert_eq!(zero["text"], json!("0"));
        let huge = preview_read_answer(&params(Some(0), Some(usize::MAX)), &page);
        assert_eq!(huge["total_chars"], json!(10));
    }

    #[test]
    fn read_answer_names_the_tab_by_its_url_when_the_page_has_no_title() {
        let page = PageText {
            url: "about:blank".to_owned(),
            ..PageText::default()
        };
        let answer = preview_read_answer(&PreviewReadParams::default(), &page);
        assert_eq!(answer["title"], json!("about:blank"));
        assert_eq!(answer["total_chars"], json!(0));
        assert!(answer.get("note").is_none(), "a healthy read carries no note");
    }

    #[test]
    fn read_answer_notes_a_truncated_page() {
        let page = PageText {
            url: "https://example.com/".to_owned(),
            text: "ab".to_owned(),
            total: Some(500_000),
            truncated: true,
            ..PageText::default()
        };
        let answer = preview_read_answer(&PreviewReadParams::default(), &page);
        assert_eq!(answer["total_chars"], json!(2));
        assert_eq!(
            answer["note"],
            json!("The page text was truncated to 2 of 500000 characters by the browser helper.")
        );
    }

    /// preview-reader.ts's identity answer for a page that is still booting.
    #[test]
    fn read_not_ready_answer_matches_the_reader_identity_answer() {
        let params = PreviewReadParams {
            session_id: "s-1".to_owned(),
            start: None,
            count: None,
        };
        assert_eq!(
            preview_read_not_ready(&params, "https://example.com/"),
            json!({
                "kind": "url",
                "title": "https://example.com/",
                "url": "https://example.com/",
                "start": 0,
                "end": 0,
                "text": "",
                "total_chars": 0,
                "note": "The page has not finished loading — retry in a moment.",
            })
        );
    }

    #[test]
    fn read_windowing_counts_utf16_units_like_javascript() {
        // A family emoji is 2 UTF-16 units per code point; "😀x" is 3 units.
        let page = PageText {
            text: "😀x".to_owned(),
            ..PageText::default()
        };
        let all = preview_read_answer(&PreviewReadParams::default(), &page);
        assert_eq!(all["total_chars"], json!(3));
        let one = preview_read_answer(
            &PreviewReadParams {
                start: Some(2),
                count: Some(1),
                ..PreviewReadParams::default()
            },
            &page,
        );
        assert_eq!(one["start"], json!(2));
        assert_eq!(one["end"], json!(3));
        assert_eq!(one["text"], json!("x"));
    }

    #[test]
    fn page_text_reads_the_helpers_result_leniently() {
        let page = PageText::from_result(&json!({
            "url": "https://example.com/",
            "title": "Example",
            "text": "hello",
            "total": 5,
            "truncated": false,
        }));
        assert_eq!(page.title, "Example");
        assert_eq!(PageText::from_result(&json!({})), PageText::default());
    }

    // ── preview.act ─────────────────────────────────────────────────────────────

    fn act_params(value: Value) -> PreviewActParams {
        serde_json::from_value(value).expect("params")
    }

    #[test]
    fn act_params_map_the_verbs_the_helper_implements() {
        assert_eq!(
            preview_act_params(&act_params(json!({"session_id": "s-1", "action": "back"}))),
            Ok(Command::Back)
        );
        assert_eq!(
            preview_act_params(&act_params(json!({"session_id": "s-1", "action": "forward"}))),
            Ok(Command::Forward)
        );
        assert_eq!(
            preview_act_params(&act_params(json!({"session_id": "s-1", "action": "reload"}))),
            Ok(Command::Reload)
        );
        assert_eq!(
            preview_act_params(&act_params(
                json!({"session_id": "s-1", "action": "click", "selector": "#go"})
            )),
            Ok(Command::Click {
                selector: Some("#go".to_owned()),
                x: None,
                y: None
            })
        );
        assert_eq!(
            preview_act_params(&act_params(json!({
                "session_id": "s-1", "action": "type", "selector": "input[name=q]",
                "text": "hello", "submit": true
            }))),
            Ok(Command::Type {
                selector: "input[name=q]".to_owned(),
                text: "hello".to_owned(),
                submit: true
            })
        );
        // scroll: amount, then the ends of the page.
        assert_eq!(
            preview_act_params(&act_params(
                json!({"session_id": "s-1", "action": "scroll", "amount": -600})
            )),
            Ok(Command::Scroll {
                amount: Some(-600.0),
                to: None
            })
        );
        assert_eq!(
            preview_act_params(&act_params(
                json!({"session_id": "s-1", "action": "scroll", "to": "bottom"})
            )),
            Ok(Command::Scroll {
                amount: None,
                to: Some(ScrollTo::Bottom)
            })
        );
        // bare scroll = one screen (the helper defaults it).
        assert_eq!(
            preview_act_params(&act_params(json!({"session_id": "s-1", "action": "scroll"}))),
            Ok(Command::Scroll { amount: None, to: None })
        );
    }

    #[test]
    fn act_params_refuse_the_unported_verbs() {
        for action in ["elements", "hover", "press", "strobe", "pin", "unpin", "hold"] {
            let message = preview_act_params(&act_params(
                json!({"session_id": "s-1", "action": action, "ref": "btn-sign-in"}),
            ))
            .expect_err(action);
            assert!(
                message.starts_with(&format!("action '{action}' is not supported")),
                "{message}"
            );
        }
        // …and answer with the pane's failure shape.
        assert_eq!(preview_act_error("no"), json!({"error": "no", "success": false}));
    }

    #[test]
    fn act_params_want_a_selector_not_a_ref() {
        let by_ref = preview_act_params(&act_params(json!({
            "session_id": "s-1", "action": "click", "ref": "btn-sign-in"
        })))
        .expect_err("ref");
        assert!(by_ref.contains("click by ref ('btn-sign-in')"), "{by_ref}");
        assert!(by_ref.contains("pass a CSS selector"), "{by_ref}");

        let blind =
            preview_act_params(&act_params(json!({"session_id": "s-1", "action": "click"}))).expect_err("no target");
        assert!(blind.contains("click needs a CSS selector"), "{blind}");

        // A ref wins no argument even when a selector is present: drive_preview sends one
        // or the other, never both, and the selector is what this helper can use.
        assert_eq!(
            preview_act_params(&act_params(json!({
                "session_id": "s-1", "action": "type", "ref": "inp-email", "selector": "#email", "text": "a@b.c"
            }))),
            Ok(Command::Type {
                selector: "#email".to_owned(),
                text: "a@b.c".to_owned(),
                submit: false
            })
        );
    }

    #[test]
    fn act_params_validate_the_scroll_target() {
        let bad = preview_act_params(&act_params(
            json!({"session_id": "s-1", "action": "scroll", "to": "middle"}),
        ))
        .expect_err("to");
        assert_eq!(bad, "to must be one of: top, bottom (got 'middle').");
    }

    /// The exact answers preview-act.ts produces, for the verbs the helper implements.
    #[test]
    fn act_answer_matches_the_pane() {
        let answered = |result: Value| Answer {
            id: 5,
            ok: true,
            result: Some(result),
            error: None,
        };

        // back / forward / reload: `{acted, note, success}` — NAV_ACTIONS in preview-act.ts.
        for (command, acted) in [
            (Command::Back, "back"),
            (Command::Forward, "forward"),
            (Command::Reload, "reload"),
        ] {
            assert_eq!(
                preview_act_answer(&command, &answered(json!({}))),
                json!({
                    "acted": acted,
                    "note": "Page is loading — call elements to see what is on it.",
                    "success": true,
                })
            );
        }

        // click: 'clicked ' + describe(target), plus the live url/title the pane's
        // post-action inventory carries.
        assert_eq!(
            preview_act_answer(
                &Command::Click {
                    selector: Some("#go".to_owned()),
                    x: None,
                    y: None
                },
                &answered(json!({
                    "tag": "button", "label": "Sign in",
                    "url": "https://example.com/app", "title": "App"
                }))
            ),
            json!({
                "acted": "clicked button \"Sign in\"",
                "url": "https://example.com/app",
                "title": "App",
                "success": true,
            })
        );

        // type: 'typed into ' + target (+ ' and submitted').
        assert_eq!(
            preview_act_answer(
                &Command::Type {
                    selector: "#q".to_owned(),
                    text: "hi".to_owned(),
                    submit: true
                },
                &answered(json!({"tag": "input", "label": "Search", "submitted": true}))
            ),
            json!({
                "acted": "typed into input \"Search\" and submitted",
                "success": true,
            })
        );

        // scroll: acted 'scrolled the page'; the empty page gets the pane's note.
        assert_eq!(
            preview_act_answer(
                &Command::Scroll {
                    amount: Some(600.0),
                    to: None
                },
                &answered(json!({"range": 4000}))
            ),
            json!({ "acted": "scrolled the page", "success": true })
        );
        assert_eq!(
            preview_act_answer(
                &Command::Scroll { amount: None, to: None },
                &answered(json!({"range": 0}))
            ),
            json!({
                "acted": "scrolled the page",
                "note": "The page has nothing to scroll — it all fits already.",
                "success": true,
            })
        );

        // A failure inside the page is the pane's `{error, success:false}`.
        assert_eq!(
            preview_act_answer(
                &Command::Back,
                &Answer {
                    id: 6,
                    ok: false,
                    result: None,
                    error: Some("no element matches #go".to_owned()),
                }
            ),
            json!({ "error": "no element matches #go", "success": false })
        );
    }

    #[test]
    fn nothing_open_strings_match_the_pane() {
        assert_eq!(NOTHING_OPEN_VALUE, "");
        assert_eq!(
            NOTHING_OPEN_ERROR,
            "No live page is open in the in-app browser — open one with open_preview first."
        );
    }
    #[test]
    fn preview_targets_get_a_scheme() {
        assert_eq!(preview_target_url("https://example.com/a"), "https://example.com/a");
        assert_eq!(preview_target_url("localhost:5173"), "http://localhost:5173");
        assert_eq!(preview_target_url("127.0.0.1:8000/x"), "http://127.0.0.1:8000/x");
        assert_eq!(preview_target_url("example.com"), "https://example.com");
        assert_eq!(preview_target_url("/tmp/my page.html"), "file:///tmp/my%20page.html");
        assert_eq!(preview_target_url("file:///tmp/a.html"), "file:///tmp/a.html");
    }

    #[test]
    fn server_answers_wrap_json_in_value() {
        assert_eq!(server_request_answer(None), json!({"value": ""}));
        let a = server_request_answer(Some(&json!({"success": true})));
        assert_eq!(a, json!({"value": "{\"success\":true}"}));
    }
}
