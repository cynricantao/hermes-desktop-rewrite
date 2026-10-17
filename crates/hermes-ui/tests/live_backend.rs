//! Live end-to-end test: the real UI (Slint testing backend, no window, no display) driven
//! against a real Hermes backend. Covers the PARITY items that need a live run:
//! composer keys (Enter / Shift+Enter / IME composition), Stop mid-stream (button and Esc),
//! model switching mid-session, the approval card (Deny / Allow once / Allow for session /
//! Always allow, by click and by keyboard), a long (600-message) conversation, and the
//! settings panel (registry nav, "Applies to" profile scope, saving, persistence, search).
//!
//! `HERMES_LIVE_ONLY=keys,stop,model,approval,long,sessions,settings,statusbar` runs a subset. The
//! `long` section
//! needs a seeded session: `HERMES_HOME=<private home> scripts/seed-long-session.py 600`. The
//! `sessions` section (open / switch / new chat / Ctrl+Tab, continue an old chat, switch while
//! streaming) needs three small labelled ones:
//! `for l in SessA SessB SessC; do HERMES_HOME=<private home> scripts/seed-long-session.py 6 $l; done`.
//! `HERMES_LIVE_NO_MODEL=1` skips its two model-call checks.
//!
//! Ignored by default: it needs a running backend and spends a few cheap model calls.
//! Run against a *private* backend with approvals enabled (never your everyday one):
//!
//! ```sh
//! scripts/live-test-backend.sh   # prints HERMES_DESKTOP_PORT=…
//! SLINT_EMIT_DEBUG_INFO=1 HERMES_DESKTOP_PORT=<port> cargo test -p hermes-ui --test live_backend -- --ignored --nocapture
//! ```
//!
//! `settings` writes through the panel and must never touch real files, so it points the app
//! at scratch copies: `HERMES_DESKTOP_PREFS` and `HERMES_DESKTOP_EXPORT_DIR` are set to a
//! temporary directory before the controller starts (the config writes go to the private
//! backend's own home).
//!
//! The approval part runs `rm -r` on throwaway directories under
//! `$HERMES_LIVE_SCRATCH` (default `$TMPDIR/hermes-live-test`). "Always allow" writes
//! `command_allowlist` into the private backend's config.yaml
//! (`$HERMES_LIVE_HOME`, default `~/.cache/hermes-desktop-rs/live-home`, which
//! `scripts/live-test-backend.sh` regenerates on every start).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use hermes_ui::{AppState, AppWindow, SettingsState};
use i_slint_backend_testing::ElementHandle;
use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{ComponentHandle, Model, SharedString};

type Log = Rc<RefCell<Vec<(String, bool, String)>>>;

fn record(log: &Log, name: &str, ok: bool, detail: impl Into<String>) {
    let detail = detail.into();
    println!("[{}] {name}: {detail}", if ok { "PASS" } else { "FAIL" });
    log.borrow_mut().push((name.into(), ok, detail));
}

async fn sleep(ms: u64) {
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    slint::Timer::single_shot(Duration::from_millis(ms), move || {
        let _ = tx.send(());
    });
    let _ = rx.await;
}

async fn wait_until(secs: u64, mut cond: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if cond() {
            return true;
        }
        sleep(100).await;
    }
    cond()
}

fn key(ui: &AppWindow, text: impl Into<SharedString>, pressed: bool) {
    let text = text.into();
    let ev = if pressed {
        WindowEvent::KeyPressed { text }
    } else {
        WindowEvent::KeyReleased { text }
    };
    ui.window().dispatch_event(ev);
}

fn type_text(ui: &AppWindow, s: &str) {
    for c in s.chars() {
        key(ui, c.to_string(), true);
        key(ui, c.to_string(), false);
    }
}

fn press(ui: &AppWindow, k: Key) {
    key(ui, k, true);
    key(ui, k, false);
}

/// Input-method composition, the way the winit backend delivers `Ime::Preedit` / `Ime::Commit`.
fn ime(ui: &AppWindow, preedit: Option<&str>, commit: Option<&str>) {
    use i_slint_core::input::{InternalKeyEvent, KeyEventType};
    use slint::platform::InternalEvent;
    if let Some(p) = preedit {
        let ev = InternalKeyEvent {
            event_type: KeyEventType::UpdateComposition,
            preedit_text: p.into(),
            ..Default::default()
        };
        ui.window()
            .dispatch_event(WindowEvent::internal(InternalEvent::Key(ev)));
    }
    if let Some(c) = commit {
        let mut ev = InternalKeyEvent {
            event_type: KeyEventType::CommitComposition,
            ..Default::default()
        };
        ev.key_event.text = c.into();
        ui.window()
            .dispatch_event(WindowEvent::internal(InternalEvent::Key(ev)));
    }
}

fn by_label(ui: &AppWindow, label: &str) -> Option<ElementHandle> {
    ElementHandle::find_by_accessible_label(ui, label).next()
}

fn composer_text(ui: &AppWindow) -> String {
    by_label(ui, "Message")
        .and_then(|e| e.accessible_value())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

fn rows(ui: &AppWindow) -> Vec<hermes_ui::ChatRow> {
    ui.global::<AppState>().get_rows().iter().collect()
}

fn last_assistant(ui: &AppWindow) -> String {
    rows(ui)
        .iter()
        .rev()
        .find(|r| r.kind == 1)
        .map(|r| r.text.to_string())
        .unwrap_or_default()
}

fn running(ui: &AppWindow) -> bool {
    ui.global::<AppState>().get_running()
}

fn idle_clean(ui: &AppWindow) -> Result<(), String> {
    let st = ui.global::<AppState>();
    let r = rows(ui);
    if st.get_running() {
        return Err("AppState.running still true".into());
    }
    if r.iter().any(|r| r.streaming) {
        return Err("a row is still marked streaming".into());
    }
    if r.iter().any(|r| r.tools_running) {
        return Err("a tool row still shows its spinner".into());
    }
    if by_label(ui, "Stop").is_some() {
        return Err("the send button still says Stop".into());
    }
    Ok(())
}

/// Send a prompt by setting the composer text and pressing Enter, then wait for the turn.
async fn send_and_wait(ui: &AppWindow, prompt: &str, secs: u64) -> bool {
    let before = rows(ui).len();
    if let Some(input) = by_label(ui, "Message") {
        input.set_accessible_value(prompt);
    }
    press(ui, Key::Return);
    wait_until(10, || rows(ui).len() > before).await;
    sleep(300).await;
    wait_until(secs, || !running(ui) && rows(ui).len() > before + 1).await
}

fn approval_visible(ui: &AppWindow) -> bool {
    ui.global::<AppState>().get_approval().visible
}

fn want(section: &str) -> bool {
    std::env::var("HERMES_LIVE_ONLY")
        .map(|v| v.split(',').any(|x| x.trim() == section))
        .unwrap_or(true)
}

fn tmpdir() -> String {
    std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into())
}

/// Resident set size of this process (the test runs the real UI in-process).
fn rss_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse().ok())
        })
        .unwrap_or(0)
}

/// Number of transcript rows Slint has actually instantiated (virtualisation check).
/// Needs `SLINT_EMIT_DEBUG_INFO=1` at build time.
fn live_message_rows(ui: &AppWindow) -> usize {
    ElementHandle::find_by_element_type_name(ui, "MessageRow").count()
}

enum Act {
    Click(&'static str),
    Key(Key),
}

struct Case {
    saw_card: bool,
    card: hermes_ui::ApprovalInfo,
    hidden_after: bool,
    done: bool,
    exists: bool,
    reply: String,
}

/// Ask the agent to `rm -r` a fresh throwaway dir. If the approval card shows, answer it
/// with `act` (or Deny when no card was expected), then wait for the turn to finish.
async fn approval_case(ui: &AppWindow, dir: &str, act: Option<Act>) -> Case {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(format!("{dir}/file.txt"), "throwaway").unwrap();
    let prompt = format!(
        "Use the terminal tool to run exactly this command and nothing else: rm -r {dir} \
         — If the command is denied, do not try any alternative; just tell me it was denied."
    );
    let before = rows(ui).len();
    by_label(ui, "Message").unwrap().set_accessible_value(prompt.as_str());
    press(ui, Key::Return);
    let reply_after = |ui: &AppWindow| {
        rows(ui)
            .iter()
            .skip(before + 1)
            .rev()
            .find(|r| r.kind == 1 && !r.text.trim().is_empty())
            .map(|r| r.text.to_string())
    };
    let mut c = Case {
        saw_card: false,
        card: Default::default(),
        hidden_after: true,
        done: false,
        exists: true,
        reply: String::new(),
    };
    let end = Instant::now() + Duration::from_secs(180);
    while Instant::now() < end {
        if approval_visible(ui) && !c.saw_card {
            c.saw_card = true;
            c.card = ui.global::<AppState>().get_approval();
            sleep(300).await;
            match &act {
                Some(Act::Click(label)) => {
                    if let Some(b) = by_label(ui, label) {
                        b.single_click(PointerEventButton::Left).await;
                    }
                }
                Some(Act::Key(k)) => press(ui, *k),
                None => {
                    if let Some(b) = by_label(ui, "Deny") {
                        b.single_click(PointerEventButton::Left).await;
                    }
                }
            }
            c.hidden_after = wait_until(5, || !approval_visible(ui)).await;
        }
        if rows(ui).len() > before + 1 && !running(ui) && reply_after(ui).is_some() {
            c.done = true;
            break;
        }
        sleep(150).await;
    }
    c.exists = std::path::Path::new(dir).exists();
    c.reply = reply_after(ui).unwrap_or_default().trim().chars().take(200).collect();
    c
}

async fn script(ui: slint::Weak<AppWindow>, log: Log) {
    let ui = ui.upgrade().expect("window");
    let st = ui.global::<AppState>();

    // --- connect
    let ok = wait_until(30, || st.get_connection() == "open").await;
    record(&log, "connect", ok, format!("connection = {}", st.get_connection()));
    if !ok {
        return;
    }

    if want("failure_recovery") {
        failure_recovery_section(&ui, &log).await;
    }
    if want("model_controls") {
        model_controls_section(&ui, &log).await;
    }
    if want("keys") {
        keys_section(&ui, &log).await;
    }
    if want("stop") {
        stop_section(&ui, &log).await;
    }
    if want("model") {
        model_section(&ui, &log).await;
    }
    if want("approval") {
        approval_section(&ui, &log).await;
    }
    if want("long") {
        long_section(&ui, &log).await;
    }
    if want("sessions") {
        sessions_section(&ui, &log).await;
    }
    if want("daily") {
        daily_section(&ui, &log).await;
    }
    if want("settings") {
        settings_section(&ui, &log).await;
    }
    if want("statusbar") {
        statusbar_section(&ui, &log).await;
    }
}

/// An independent RPC read verifies that UI selections reached the real private backend.
async fn backend_request(method: &str, params: serde_json::Value) -> Result<serde_json::Value, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let method = method.to_owned();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(async {
            let backend = hermes_core::backend::discover_or_spawn(None)
                .await
                .map_err(|e| e.to_string())?;
            let (client, _events) = hermes_core::Client::connect(&backend.endpoint.ws_url())
                .await
                .map_err(|e| e.to_string())?;
            client.request(&method, params).await.map_err(|e| e.to_string())
        });
        let _ = tx.send(result);
    });
    rx.await.map_err(|e| e.to_string())?
}

async fn click(ui: &AppWindow, label: &str) -> bool {
    // Headless snapshots do not present frames automatically. Drain layout/property changes
    // before computing a new menu/row's pointer target, as a visible frame would.
    sleep(150).await;
    if let Some(element) = by_label(ui, label) {
        println!(
            "[CLICK] {label:?} position={:?} size={:?}",
            element.absolute_position(),
            element.size()
        );
        element.single_click(PointerEventButton::Left).await;
        true
    } else {
        false
    }
}

async fn failure_recovery_section(ui: &AppWindow, log: &Log) {
    use serde_json::json;
    let st = ui.global::<AppState>();
    st.invoke_new_chat();
    sleep(200).await;
    st.invoke_send("/help".into());
    let ready = wait_until(15, || !st.get_runtime_session().is_empty()).await;
    let ready = ready && wait_until(15, || st.get_rows().row_count() > 0).await;
    let sid = st.get_runtime_session().to_string();
    let closed = if ready {
        backend_request("session.close", json!({"session_id":sid})).await.ok()
    } else {
        None
    };
    if closed
        .as_ref()
        .and_then(|r| r.get("closed"))
        .and_then(serde_json::Value::as_bool)
        != Some(true)
    {
        record(
            log,
            "failure recovery: close isolated fixture before probing",
            false,
            format!("ready={ready}, closed={closed:?}"),
        );
        return; // Never send a normal prompt unless the runtime is confirmed absent.
    }
    let text = "Unsent text recovered after a rejected submission";
    st.set_draft("".into());
    st.invoke_send(text.into());
    let failed = wait_until(10, || {
        st.get_banner().starts_with("Message failed:") && !st.get_running() && st.get_draft() == text
    })
    .await;
    record(
        log,
        "failure recovery: rejected send shows an error, clears busy state and restores typed text",
        failed && st.get_banner_error(),
        format!(
            "banner={}, running={}, restored={}",
            st.get_banner(),
            st.get_running(),
            st.get_draft() == text
        ),
    );
    screenshot(ui, "daily-send-failure");
    st.invoke_stop();
    let failed = wait_until(10, || st.get_banner().starts_with("Could not stop chat:")).await;
    record(
        log,
        "failure recovery: rejected Stop is visible rather than ignored",
        failed && st.get_banner_error(),
        st.get_banner().to_string(),
    );
    st.invoke_dismiss_banner();
    let dismissed = wait_until(5, || st.get_banner().is_empty()).await;
    st.invoke_new_chat();
    let clean = wait_until(5, || {
        st.get_runtime_session().is_empty() && st.get_draft().is_empty() && !st.get_running()
    })
    .await;
    record(
        log,
        "failure recovery: dismiss and new chat recover cleanly",
        dismissed && clean,
        format!("dismissed={dismissed}, clean={clean}"),
    );
}

async fn model_controls_section(ui: &AppWindow, log: &Log) {
    let st = ui.global::<AppState>();
    st.invoke_new_chat();
    wait_until(10, || {
        !st.get_model_label().is_empty() && st.get_model_label() != "Model"
    })
    .await;
    let label = format!("{} ", st.get_model_label());
    let clicked = click(ui, &label).await;
    let loaded = wait_until(10, || ui.get_picker_open() && st.get_models().iter().any(|m| !m.header)).await;
    record(
        log,
        "model controls: clicking the model chip opens the backend catalog",
        clicked && loaded,
        format!(
            "clicked={clicked}, open={}, rows={}",
            ui.get_picker_open(),
            st.get_models().row_count()
        ),
    );
    let model = st.get_models().iter().find(|m| !m.header && !m.current);
    if let Some(model) = model {
        let clicked = click(ui, &format!("Use {}", model.model)).await;
        let expected = model
            .model
            .rsplit('/')
            .next()
            .unwrap_or(model.model.as_str())
            .to_owned();
        let applied = wait_until(5, || !ui.get_picker_open() && st.get_model_label() == expected).await;
        record(
            log,
            "model controls: clicking a model changes the draft selection",
            clicked && applied && st.get_runtime_session().is_empty(),
            format!(
                "clicked={clicked}, model={}, no inference/session started={}",
                st.get_model_label(),
                st.get_runtime_session().is_empty()
            ),
        );
    } else {
        record(
            log,
            "model controls: alternate configured-provider model available",
            false,
            "catalog did not offer an alternate model",
        );
    }
    st.invoke_new_chat();
    wait_until(5, || st.get_model_label().as_str() == label.trim_end()).await;
    st.invoke_send("/model".into());
    let opened = wait_until(5, || ui.get_picker_open()).await;
    record(
        log,
        "model controls: /model opens the same menu without calling a model",
        opened && st.get_runtime_session().is_empty(),
        format!("open={opened}, runtime={}", st.get_runtime_session()),
    );
    ui.set_picker_open(false);
}

fn screenshot(ui: &AppWindow, name: &str) {
    let Some(dir) = std::env::var_os("HERMES_LIVE_SCREENSHOTS") else {
        return;
    };
    use std::io::Write;
    let buffer = ui.window().take_snapshot().expect("headless software snapshot");
    std::fs::create_dir_all(&dir).unwrap();
    let path = std::path::PathBuf::from(dir).join(format!("{name}.ppm"));
    let mut file = std::fs::File::create(&path).unwrap();
    writeln!(file, "P6\n{} {}\n255", buffer.width(), buffer.height()).unwrap();
    for pixel in buffer.as_bytes().as_chunks::<4>().0 {
        file.write_all(&pixel[..3]).unwrap();
    }
    println!("[SCREENSHOT] {}", path.display());
}

async fn daily_section(ui: &AppWindow, log: &Log) {
    use hermes_ui::ProjectState;
    use serde_json::json;
    let st = ui.global::<AppState>();
    st.invoke_new_chat();
    wait_until(5, || st.get_runtime_session().is_empty()).await;
    let clicked = click(ui, "Projects").await;
    let ps = ui.global::<ProjectState>();
    let opened = wait_until(5, || ps.get_open() && !ps.get_loading()).await;
    record(
        log,
        "daily: Projects button opens the project dialog",
        clicked && opened,
        format!("clicked={clicked}, open={}", ps.get_open()),
    );

    ps.set_name("Invalid folder".into());
    ps.set_path("/does-not-exist/hermes-project".into());
    let clicked = click(ui, "Add project").await;
    let rejected = wait_until(5, || ps.get_error().contains("does not exist")).await;
    record(
        log,
        "daily: missing project folder is rejected without creating it",
        clicked && rejected && !std::path::Path::new("/does-not-exist/hermes-project").exists(),
        ps.get_error().to_string(),
    );

    let folder = std::env::temp_dir().join(format!("hermes-daily-project-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let label = format!("Daily project {}", std::process::id());
    ps.set_name(label.clone().into());
    ps.set_path(folder.to_string_lossy().to_string().into());
    click(ui, "Add project").await;
    let saved = wait_until(10, || {
        ps.get_items().iter().any(|p| p.name == label) && ps.get_name().is_empty() && ps.get_error().is_empty()
    })
    .await;
    let project = ps.get_items().iter().find(|p| p.name == label);
    let listed = backend_request("projects.list", json!({})).await.unwrap();
    let persisted = listed["projects"]
        .as_array()
        .is_some_and(|ps| ps.iter().any(|p| p["name"] == label));
    record(
        log,
        "daily: Add project persists in the backend registry",
        saved && persisted && ps.get_error().is_empty() && ps.get_name().is_empty(),
        format!("UI={saved}, independent read={persisted}"),
    );
    let Some(project) = project else {
        return;
    };

    screenshot(ui, "daily-projects");
    let clicked = click(ui, &label).await;
    let selected = wait_until(10, || ps.get_active_id() == project.id && !ps.get_open()).await;
    let listed = backend_request("projects.list", json!({})).await.unwrap();
    record(
        log,
        "daily: clicking a project selects its folder and starts a clean draft",
        clicked && selected && listed["active_id"] == project.id.as_str() && st.get_rows().row_count() == 0,
        format!("cwd label={}, active_id={}", st.get_cwd_label(), listed["active_id"]),
    );

    // Click the actual reasoning control and one option, not just the global callback.
    let effort_label = format!("{} ", st.get_effort_label());
    let clicked = click(ui, &effort_label).await;
    wait_until(5, || ui.get_effort_open()).await;
    sleep(100).await;
    screenshot(ui, "daily-reasoning");
    let option = click(ui, "max").await;
    let selected = wait_until(5, || st.get_effort_label() == "max").await;
    record(
        log,
        "daily: reasoning pill opens and max is selectable before chat",
        clicked && option && selected,
        format!("pill={clicked}, option={option}, effort={}", st.get_effort_label()),
    );

    st.invoke_select_model("openai".into(), "gpt-4o-mini".into());
    wait_until(5, || st.get_model_label() == "gpt-4o-mini").await;
    // /help creates the actual session but does not ask a model anything.
    st.invoke_send("/help".into());
    let created = wait_until(15, || !st.get_runtime_session().is_empty()).await;
    let sid = st.get_runtime_session().to_string();
    let reasoning = backend_request("config.get", json!({"key":"reasoning", "session_id":sid}))
        .await
        .unwrap();
    let models = backend_request("model.options", json!({"session_id":sid}))
        .await
        .unwrap();
    record(
        log,
        "daily: draft model and reasoning reach session.create",
        created && reasoning["value"] == "max" && models["model"] == "gpt-4o-mini",
        format!("reasoning={}, model={}", reasoning["value"], models["model"]),
    );
    let project_info = backend_request("projects.for_cwd", json!({"cwd":folder}))
        .await
        .unwrap();
    record(
        log,
        "daily: selected project is associated with its execution folder",
        created
            && st
                .get_cwd_label()
                .ends_with(folder.file_name().unwrap().to_string_lossy().as_ref())
            && project_info["project"]["id"] == project.id.as_str(),
        format!("cwd={}", st.get_cwd_label()),
    );

    st.invoke_select_effort("high".into());
    let updated = wait_until(10, || st.get_effort_label() == "high").await;
    let reasoning = backend_request("config.get", json!({"key":"reasoning", "session_id":sid}))
        .await
        .unwrap();
    record(
        log,
        "daily: reasoning change updates the live session",
        updated && reasoning["value"] == "high",
        format!("backend={}", reasoning["value"]),
    );
    let config_path = std::path::PathBuf::from(std::env::var("HERMES_HOME").unwrap()).join("config.yaml");
    let config = std::fs::read_to_string(config_path).unwrap();
    record(
        log,
        "daily: per-chat reasoning does not change the global default",
        config.contains("reasoning_effort: medium"),
        "private config still has reasoning_effort: medium",
    );

    ps.set_open(true);
    ps.invoke_refresh();
    wait_until(5, || !ps.get_loading()).await;
    click(ui, &format!("Edit {label}")).await;
    let renamed = format!("{label} renamed");
    ps.set_name(renamed.clone().into());
    click(ui, "Save project").await;
    let saved = wait_until(10, || ps.get_items().iter().any(|p| p.name == renamed)).await;
    let read = backend_request("projects.get", json!({"id":project.id.as_str()}))
        .await
        .unwrap();
    record(
        log,
        "daily: rename is persisted and read back",
        saved && read["project"]["name"] == renamed,
        read["project"]["name"].to_string(),
    );
    // A seeded, clearly labelled fixture exercises project + draft restoration without inference.
    let fixture = backend_request("session.create", json!({
        "source":"desktop", "cwd":folder, "cwd_explicit":true,
        "title":"Daily project restore fixture",
        "messages":[{"role":"user","content":"Seeded project question"}, {"role":"assistant","content":"Seeded fixture answer, not a model response."}]
    })).await.unwrap();
    let fixture_id = fixture["stored_session_id"].as_str().unwrap().to_owned();
    ps.set_open(false);
    st.invoke_open_session(fixture_id.clone().into());
    let resumed = wait_until(10, || {
        st.get_active_session() == fixture_id && ps.get_active_id() == project.id
    })
    .await;
    record(
        log,
        "daily: resuming a project chat restores its project scope",
        resumed,
        format!("project={}, cwd={}", ps.get_active_id(), st.get_cwd_label()),
    );
    st.set_draft("Unsent text belonging to the project chat".into());
    sleep(150).await;
    if let Some(other) = st.get_sessions().iter().find(|s| s.title.contains("SessA")) {
        st.invoke_open_session(other.id.clone());
        let switched = wait_until(10, || st.get_active_session() == other.id && st.get_draft().is_empty()).await;
        st.invoke_open_session(fixture_id.clone().into());
        let restored = wait_until(10, || {
            st.get_active_session() == fixture_id && st.get_draft() == "Unsent text belonging to the project chat"
        })
        .await;
        record(
            log,
            "daily: unsent text stays with its own chat across switching",
            switched && restored,
            format!("switched={switched}, restored={restored}"),
        );
    } else {
        record(log, "daily: unsent text fixture available", false, "SessA missing");
    }
    st.invoke_new_chat();
    let inherited = wait_until(10, || {
        st.get_active_session().is_empty()
            && ps.get_active_id() == project.id
            && st
                .get_cwd_label()
                .ends_with(folder.file_name().unwrap().to_string_lossy().as_ref())
    })
    .await;
    record(
        log,
        "daily: new chat after project resume keeps the project folder",
        inherited,
        format!("project={}, cwd={}", ps.get_active_id(), st.get_cwd_label()),
    );
    ps.set_open(true);
    click(ui, "Chat without a project").await;
    let cleared = wait_until(10, || {
        ps.get_active_id().is_empty() && st.get_runtime_session().is_empty()
    })
    .await;
    wait_until(5, || st.get_effort_label() == "medium").await;
    record(
        log,
        "daily: leaving the project resets draft model/reasoning to defaults",
        cleared && st.get_effort_label() == "medium",
        format!("active={}, reasoning={}", ps.get_active_id(), st.get_effort_label()),
    );

    // The seeded fixtures let us check real resume/switch without a model call.
    let sessions: Vec<_> = st.get_sessions().iter().filter(|s| s.title.contains("Sess")).collect();
    if sessions.len() >= 2 {
        st.invoke_open_session(sessions[0].id.clone());
        wait_until(10, || st.get_active_session() == sessions[0].id).await;
        st.invoke_select_effort("low".into());
        wait_until(5, || st.get_effort_label() == "low").await;
        st.invoke_open_session(sessions[1].id.clone());
        wait_until(10, || st.get_active_session() == sessions[1].id).await;
        st.invoke_open_session(sessions[0].id.clone());
        let restored = wait_until(10, || {
            st.get_active_session() == sessions[0].id && st.get_effort_label() == "low"
        })
        .await;
        record(
            log,
            "daily: reopening a chat restores its reasoning rather than the other chat's",
            restored,
            format!("reasoning={}", st.get_effort_label()),
        );
    } else {
        record(
            log,
            "daily: seeded chats available for restoration",
            false,
            format!("seeded={}", sessions.len()),
        );
    }
    st.invoke_new_chat();
    wait_until(5, || st.get_active_session().is_empty()).await;
}

async fn keys_section(ui: &AppWindow, log: &Log) {
    let (ui, log) = (ui.clone_strong(), log.clone());
    if let Some(input) = by_label(&ui, "Message") {
        input.single_click(PointerEventButton::Left).await;
    }
    type_text(&ui, "hello");
    key(&ui, Key::Shift, true);
    press(&ui, Key::Return);
    key(&ui, Key::Shift, false);
    sleep(100).await;
    let t = composer_text(&ui);
    record(
        &log,
        "Shift+Enter inserts a newline (no send)",
        t == "hello\n" && rows(&ui).is_empty(),
        format!("composer = {t:?}, rows = {}", rows(&ui).len()),
    );

    ime(&ui, Some("にほん"), None);
    sleep(50).await;
    let mid = composer_text(&ui);
    ime(&ui, None, Some("日本"));
    sleep(100).await;
    let t = composer_text(&ui);
    record(
        &log,
        "IME composition commits text and does not send",
        t == "hello\n日本" && rows(&ui).is_empty(),
        format!(
            "during preedit = {mid:?}, after commit = {t:?}, rows = {}",
            rows(&ui).len()
        ),
    );
}

async fn stop_section(ui: &AppWindow, log: &Log) {
    let (ui, log) = (ui.clone_strong(), log.clone());
    if let Some(input) = by_label(&ui, "Message") {
        input.set_accessible_value("");
    }
    let prompt = "Write the numbers from one to three hundred as English words, one per line. No tools, no preamble.";
    by_label(&ui, "Message").unwrap().set_accessible_value(prompt);
    press(&ui, Key::Return);
    let n0 = rows(&ui).len();
    let sent = wait_until(10, || rows(&ui).get(n0).is_some_and(|r| r.kind == 0)).await;
    record(
        &log,
        "Enter sends",
        sent && composer_text(&ui).is_empty(),
        format!(
            "first row = {:?}, composer = {:?}",
            rows(&ui).get(n0).map(|r| r.text.to_string()),
            composer_text(&ui)
        ),
    );

    let streaming = wait_until(90, || last_assistant(&ui).lines().count() >= 8).await;
    let before_stop = last_assistant(&ui).lines().count();
    let stop_btn = by_label(&ui, "Stop");
    record(
        &log,
        "reply streams and button turns into Stop",
        streaming && stop_btn.is_some(),
        format!("{before_stop} lines streamed before Stop"),
    );
    if let Some(b) = stop_btn {
        b.single_click(PointerEventButton::Left).await;
    }
    let stopped = wait_until(15, || !running(&ui)).await;
    let n1 = last_assistant(&ui).lines().count();
    sleep(3000).await;
    let n2 = last_assistant(&ui).lines().count();
    let clean = idle_clean(&ui);
    record(
        &log,
        "Stop mid-stream: backend stops, UI recovers",
        stopped && n1 == n2 && n2 < 300 && clean.is_ok(),
        format!(
            "running={} lines at stop {n1}, 3 s later {n2} (of 300); {}",
            running(&ui),
            clean.err().unwrap_or_else(|| "no spinner, button back to Send".into())
        ),
    );

    let ok = send_and_wait(&ui, "Reply with exactly the word: ok", 120).await;
    let reply = last_assistant(&ui);
    record(
        &log,
        "next prompt after Stop works",
        ok && reply.to_lowercase().contains("ok"),
        format!("reply = {:?}", reply.trim()),
    );

    // Esc stops a running turn (global shortcut; focus is in the composer).
    by_label(&ui, "Message").unwrap().set_accessible_value(prompt);
    press(&ui, Key::Return);
    let base = last_assistant(&ui);
    let streaming = wait_until(90, || {
        let t = last_assistant(&ui);
        t != base && t.lines().count() >= 8
    })
    .await;
    press(&ui, Key::Escape);
    let stopped = wait_until(15, || !running(&ui)).await;
    let n1 = last_assistant(&ui).lines().count();
    sleep(3000).await;
    let n2 = last_assistant(&ui).lines().count();
    let clean = idle_clean(&ui);
    record(
        &log,
        "Esc stops mid-stream",
        streaming && stopped && n1 == n2 && n2 < 300 && clean.is_ok(),
        format!(
            "streaming={streaming}; running={} after Esc; lines at stop {n1}, 3 s later {n2} (of 300); {}",
            running(&ui),
            clean.err().unwrap_or_else(|| "no spinner, button back to Send".into())
        ),
    );
    let ok = send_and_wait(&ui, "Reply with exactly the word: ok", 120).await;
    let reply = last_assistant(&ui);
    record(
        &log,
        "next prompt after Esc works",
        ok && reply.to_lowercase().contains("ok"),
        format!("reply = {:?}", reply.trim()),
    );
}

async fn model_section(ui: &AppWindow, log: &Log) {
    let (ui, log) = (ui.clone_strong(), log.clone());
    let st = ui.global::<AppState>();
    // Start from a fresh draft (no live session yet): the switch must still apply.
    st.invoke_new_chat();
    wait_until(5, || rows(&ui).is_empty()).await;
    st.invoke_refresh_models();
    wait_until(20, || st.get_models().row_count() > 0).await;
    let models: Vec<_> = st.get_models().iter().filter(|m| !m.header).collect();
    let current = models.iter().find(|m| m.current).cloned();
    let target = current.as_ref().and_then(|c| {
        models
            .iter()
            .find(|m| m.provider == c.provider && !m.current && m.model.contains("glm-5.3-flash"))
            .or_else(|| models.iter().find(|m| m.provider == c.provider && !m.current))
            .cloned()
    });
    match (current, target) {
        (Some(c), Some(t)) => {
            let label_before = st.get_model_label();
            st.invoke_select_model(t.provider.clone(), t.model.clone());
            let short = t.model.rsplit('/').next().unwrap_or(&t.model).to_string();
            let switched = wait_until(20, || st.get_model_label() == short.as_str()).await;
            let ok = send_and_wait(&ui, "Reply with exactly the word: pong", 120).await;
            let reply = last_assistant(&ui);
            record(
                &log,
                "model pick on a new chat (rides on session.create)",
                switched && ok && reply.to_lowercase().contains("pong") && st.get_model_label() == short.as_str(),
                format!(
                    "{} → {} (label {label_before} → {}); reply after switch = {:?}; banner = {:?}",
                    c.model,
                    t.model,
                    st.get_model_label(),
                    reply.trim(),
                    st.get_banner().to_string()
                ),
            );
            // Now switch back mid-session (config.set on the live session).
            let back = c.model.rsplit('/').next().unwrap_or(&c.model).to_string();
            st.invoke_select_model(c.provider.clone(), c.model.clone());
            let switched = wait_until(30, || st.get_model_label() == back.as_str()).await;
            let ok = send_and_wait(&ui, "Reply with exactly the word: pong", 120).await;
            let reply = last_assistant(&ui);
            record(
                &log,
                "model switch mid-session",
                switched && ok && reply.to_lowercase().contains("pong") && st.get_banner().is_empty(),
                format!(
                    "{} → {} (label now {}); reply = {:?}; banner = {:?}",
                    t.model,
                    c.model,
                    st.get_model_label(),
                    reply.trim(),
                    st.get_banner().to_string()
                ),
            );
        }
        (c, t) => record(
            &log,
            "model switch mid-session",
            false,
            format!("no target model (current {c:?}, target {t:?})"),
        ),
    }
}

async fn approval_section(ui: &AppWindow, log: &Log) {
    let (ui, log) = (ui.clone_strong(), log.clone());
    let st = ui.global::<AppState>();
    let scratch = std::env::var("HERMES_LIVE_SCRATCH").unwrap_or_else(|_| format!("{}/hermes-live-test", tmpdir()));
    let live_home = live_home();
    // "Always allow" persists in the private config; a second run on the same backend would
    // never see a card. live-test-backend.sh regenerates config.yaml on every start.
    let cfg_path = format!("{live_home}/config.yaml");
    if std::fs::read_to_string(&cfg_path)
        .unwrap_or_default()
        .contains("command_allowlist")
    {
        record(
            &log,
            "approval precondition",
            false,
            format!(
                "{cfg_path} already has command_allowlist (from an earlier run): restart scripts/live-test-backend.sh"
            ),
        );
        return;
    }
    // Fresh chat so earlier "session" approvals can't leak in.
    st.invoke_new_chat();
    wait_until(5, || rows(&ui).is_empty()).await;

    // (name, act, expect dir removed). Keyboard cases come before "session", which would
    // otherwise pre-approve the pattern for the rest of this chat.
    let cases: [(&str, Act, bool); 4] = [
        ("Deny (click)", Act::Click("Deny"), false),
        ("Allow once (click)", Act::Click("Allow once"), true),
        ("Esc denies", Act::Key(Key::Escape), false),
        ("Enter allows once", Act::Key(Key::Return), true),
    ];
    for (i, (name, act, removed)) in cases.into_iter().enumerate() {
        let dir = format!("{scratch}/victim-{i}");
        let c = approval_case(&ui, &dir, Some(act)).await;
        record(
            &log,
            &format!("approval card: {name}"),
            c.saw_card && c.hidden_after && c.done && c.exists != removed && idle_clean(&ui).is_ok(),
            format!(
                "card shown={} command={:?} choices: session={} always={}; hidden={}; turn done={}; dir exists={}; agent: {:?}",
                c.saw_card,
                c.card.command.to_string(),
                c.card.allow_session,
                c.card.allow_always,
                c.hidden_after,
                c.done,
                c.exists,
                c.reply
            ),
        );
    }

    // Allow for session: approves this chat's pattern, so the next rm -r runs with no card.
    let a = approval_case(
        &ui,
        &format!("{scratch}/victim-session-a"),
        Some(Act::Click("Allow for session")),
    )
    .await;
    let b = approval_case(&ui, &format!("{scratch}/victim-session-b"), None).await;
    record(
        &log,
        "approval card: Allow for session",
        a.saw_card && a.card.allow_session && a.done && !a.exists && !b.saw_card && b.done && !b.exists,
        format!(
            "1st: card={} removed={}; 2nd in same chat: card={} removed={} agent: {:?}",
            a.saw_card, !a.exists, b.saw_card, !b.exists, b.reply
        ),
    );
    // ...and does not leak into a new chat.
    st.invoke_new_chat();
    wait_until(5, || rows(&ui).is_empty()).await;
    let c = approval_case(&ui, &format!("{scratch}/victim-session-c"), Some(Act::Click("Deny"))).await;
    record(
        &log,
        "Allow for session is scoped to that chat",
        c.saw_card && c.done && c.exists,
        format!("new chat: card={} dir kept={}", c.saw_card, c.exists),
    );

    // Always allow: persisted to the private config's command_allowlist; a new chat runs it with no card.
    st.invoke_new_chat();
    wait_until(5, || rows(&ui).is_empty()).await;
    let cfg = format!("{live_home}/config.yaml");
    let before = std::fs::read_to_string(&cfg)
        .unwrap_or_default()
        .contains("command_allowlist");
    let a = approval_case(
        &ui,
        &format!("{scratch}/victim-always-a"),
        Some(Act::Click("Always allow")),
    )
    .await;
    let after = std::fs::read_to_string(&cfg).unwrap_or_default();
    let persisted = after.contains("command_allowlist");
    st.invoke_new_chat();
    wait_until(5, || rows(&ui).is_empty()).await;
    let b = approval_case(&ui, &format!("{scratch}/victim-always-b"), None).await;
    record(
        &log,
        "approval card: Always allow",
        a.saw_card && a.card.allow_always && a.done && !a.exists && persisted && !b.saw_card && b.done && !b.exists,
        format!(
            "card={} removed={}; command_allowlist in private config: before={before} after={persisted}; \
             new chat: card={} removed={}",
            a.saw_card, !a.exists, b.saw_card, !b.exists
        ),
    );
}

async fn long_section(ui: &AppWindow, log: &Log) {
    let (ui, log) = (ui.clone_strong(), log.clone());
    let st = ui.global::<AppState>();
    wait_until(10, || st.get_sessions().row_count() > 0).await;
    let id = std::env::var("HERMES_LIVE_LONG_SESSION").ok().or_else(|| {
        st.get_sessions()
            .iter()
            .find(|s| s.title.contains("Question 0: explain item 0"))
            .map(|s| s.id.to_string())
    });
    let Some(id) = id else {
        record(
            &log,
            "long conversation",
            false,
            "no seeded session (run scripts/seed-long-session.py)",
        );
        return;
    };
    let rss0 = rss_kib();
    let t0 = Instant::now();
    st.invoke_open_session(id.as_str().into());
    let loaded = wait_until(60, || rows(&ui).len() >= 500).await;
    let load_ms = t0.elapsed().as_millis();
    sleep(1000).await;
    let n = rows(&ui).len();
    let live_rest = live_message_rows(&ui);
    let rss_loaded = rss_kib();

    // Sweep to the top and back (Transcript.autoscroll), sampling instantiated rows.
    st.set_autoscroll(true);
    let mut max_live = live_rest;
    let t1 = Instant::now();
    while st.get_autoscroll() && t1.elapsed() < Duration::from_secs(120) {
        max_live = max_live.max(live_message_rows(&ui));
        sleep(100).await;
    }
    let sweep_s = t1.elapsed().as_secs_f32();
    sleep(1000).await;
    let rss_after = rss_kib();
    // A second sweep: memory must not keep growing.
    st.set_autoscroll(true);
    while st.get_autoscroll() && t1.elapsed() < Duration::from_secs(240) {
        max_live = max_live.max(live_message_rows(&ui));
        sleep(100).await;
    }
    sleep(1000).await;
    let rss_after2 = rss_kib();
    let mib = |k: u64| k as f64 / 1024.0;
    let grow2 = rss_after2 as i64 - rss_after as i64;
    record(
        &log,
        "long conversation: virtualised, memory flat",
        loaded && n >= 500 && max_live > 0 && max_live < 80 && grow2 < 4096,
        format!(
            "{n} rows loaded in {load_ms} ms; MessageRow instances at rest {live_rest}, max during sweep {max_live}; \
             sweep {sweep_s:.1} s; RSS {:.1} → {:.1} (loaded) → {:.1} (after sweep 1) → {:.1} MiB (after sweep 2)",
            mib(rss0),
            mib(rss_loaded),
            mib(rss_after),
            mib(rss_after2)
        ),
    );
}

/// Text of every transcript row, joined (for "which chat is on screen" checks).
fn transcript_text(ui: &AppWindow) -> String {
    rows(ui)
        .iter()
        .map(|r| r.text.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Stored id of the first sidebar chat whose title contains `needle`.
fn session_id(ui: &AppWindow, needle: &str) -> Option<String> {
    ui.global::<AppState>()
        .get_sessions()
        .iter()
        .find(|s| s.title.contains(needle))
        .map(|s| s.id.to_string())
}

/// Click a sidebar chat the way a user does (its NavItem's default action).
fn click_session(ui: &AppWindow, needle: &str) -> bool {
    let Some(title) = ui
        .global::<AppState>()
        .get_sessions()
        .iter()
        .find(|s| s.title.contains(needle))
        .map(|s| s.title.to_string())
    else {
        return false;
    };
    match by_label(ui, &title) {
        Some(e) => {
            e.invoke_accessible_default_action();
            true
        }
        None => false,
    }
}

fn chord(ui: &AppWindow, mods: &[Key], k: impl Into<SharedString>) {
    for m in mods {
        key(ui, *m, true);
    }
    let k = k.into();
    key(ui, k.clone(), true);
    key(ui, k, false);
    for m in mods.iter().rev() {
        key(ui, *m, false);
    }
}

/// Open `label`'s chat by clicking it; true once its history (and only its) is on screen.
/// Opening an idle chat must show it idle. A chat whose reply is still streaming is
/// legitimately running, so pass `expect_idle = false` when coming back to one.
async fn open_and_check(ui: &AppWindow, log: &Log, label: &str, others: &[&str]) -> bool {
    open_and_check_with(ui, log, label, others, true).await
}

async fn open_and_check_with(ui: &AppWindow, log: &Log, label: &str, others: &[&str], expect_idle: bool) -> bool {
    let st = ui.global::<AppState>();
    let want_id = session_id(ui, label).unwrap_or_default();
    let t0 = Instant::now();
    let clicked = click_session(ui, label);
    let marker = format!("{label} Question 0");
    let shown = wait_until(20, || transcript_text(ui).contains(&marker)).await;
    let ms = t0.elapsed().as_millis();
    sleep(300).await;
    let text = transcript_text(ui);
    let leaked: Vec<_> = others
        .iter()
        .filter(|o| text.contains(&format!("{o} Question")))
        .collect();
    let active = st.get_active_session().to_string();
    let ok = clicked && shown && leaked.is_empty() && active == want_id && (!expect_idle || !running(ui));
    record(
        log,
        &format!("open {label}"),
        ok,
        format!(
            "clicked={clicked} history shown={shown} in {ms} ms; rows={} {:?}; other chats leaked={leaked:?}; \
             highlighted={} title={:?}",
            rows(ui).len(),
            rows(ui)
                .iter()
                .map(|r| (r.kind, r.text.chars().take(24).collect::<String>()))
                .collect::<Vec<_>>(),
            active == want_id,
            st.get_session_title()
        ),
    );
    ok
}

async fn sessions_section(ui: &AppWindow, log: &Log) {
    let (ui, log) = (ui.clone_strong(), log.clone());
    let st = ui.global::<AppState>();
    let labels = ["SessA", "SessB", "SessC"];
    let seeded = wait_until(10, || labels.iter().all(|l| session_id(&ui, l).is_some())).await;
    record(
        &log,
        "sessions: seeded chats listed",
        seeded,
        format!("{} sidebar rows", st.get_sessions().row_count()),
    );
    if !seeded {
        return;
    }

    // Open, switch, switch back (the second open reuses the live session).
    open_and_check(&ui, &log, "SessA", &["SessB", "SessC"]).await;
    open_and_check(&ui, &log, "SessB", &["SessA", "SessC"]).await;
    open_and_check(&ui, &log, "SessA", &["SessB", "SessC"]).await;

    // Ctrl+N: back to an empty new chat.
    chord(&ui, &[Key::Control], "n");
    let fresh = wait_until(5, || rows(&ui).is_empty() && st.get_active_session().is_empty()).await;
    record(
        &log,
        "Ctrl+N new chat",
        fresh && st.get_session_title() == "New chat",
        format!(
            "rows={} active={:?} title={:?}",
            rows(&ui).len(),
            st.get_active_session(),
            st.get_session_title()
        ),
    );

    // Ctrl+Tab / Ctrl+Shift+Tab walk the sidebar order (wrapping); from a new chat Ctrl+Tab opens the first.
    let order: Vec<String> = st.get_sessions().iter().map(|s| s.id.to_string()).collect();
    let mut seen = vec![];
    chord(&ui, &[Key::Control], Key::Tab);
    wait_until(20, || st.get_active_session() == order[0] && !rows(&ui).is_empty()).await;
    seen.push(st.get_active_session().to_string());
    chord(&ui, &[Key::Control], Key::Tab);
    wait_until(20, || order.len() > 1 && st.get_active_session() == order[1]).await;
    seen.push(st.get_active_session().to_string());
    chord(&ui, &[Key::Control, Key::Shift], Key::Tab);
    wait_until(20, || st.get_active_session() == order[0]).await;
    seen.push(st.get_active_session().to_string());
    chord(&ui, &[Key::Control, Key::Shift], Key::Tab);
    let last = order.last().cloned().unwrap_or_default();
    wait_until(20, || st.get_active_session() == last).await;
    seen.push(st.get_active_session().to_string());
    let expect = [
        order[0].clone(),
        order.get(1).cloned().unwrap_or_default(),
        order[0].clone(),
        last,
    ];
    record(
        &log,
        "Ctrl+Tab / Ctrl+Shift+Tab switch chats",
        seen == expect,
        format!(
            "visited positions {:?} of {}",
            seen.iter()
                .map(|s| order.iter().position(|o| o == s))
                .collect::<Vec<_>>(),
            order.len()
        ),
    );

    if std::env::var_os("HERMES_LIVE_NO_MODEL").is_some() {
        return;
    }

    // Continue an old chat: the reply lands under its history and survives a switch away and back.
    open_and_check(&ui, &log, "SessB", &["SessA", "SessC"]).await;
    let before = rows(&ui).len();
    let answered = send_and_wait(&ui, "Reply with only the word PINEAPPLE.", 120).await;
    let reply = last_assistant(&ui);
    open_and_check(&ui, &log, "SessC", &["SessA", "SessB"]).await;
    open_and_check(&ui, &log, "SessB", &["SessA", "SessC"]).await;
    // The prompt itself contains the word: look at the assistant's reply only.
    let kept = last_assistant(&ui).contains("PINEAPPLE");
    record(
        &log,
        "continue an old chat",
        // Rows must grow, but not necessarily by two: the backend drops an earlier unanswered
        // turn from a reused fixture when the next one succeeds.
        answered && reply.contains("PINEAPPLE") && kept && rows(&ui).len() > before,
        format!(
            "answered={answered} reply={reply:?} rows {before} → {}; still there after switching away and back={kept}",
            rows(&ui).len()
        ),
    );

    // Switch away while a reply streams: the other chat stays clean; coming back shows the finished reply.
    open_and_check(&ui, &log, "SessA", &["SessB", "SessC"]).await;
    if let Some(input) = by_label(&ui, "Message") {
        input.set_accessible_value("Count from 1 to 80, one number per line, then write DONE.");
    }
    press(&ui, Key::Return);
    let streaming = wait_until(60, || last_assistant(&ui).contains('5')).await;
    open_and_check(&ui, &log, "SessC", &["SessA", "SessB"]).await;
    sleep(3000).await;
    let c_clean = !transcript_text(&ui).contains("DONE") && !transcript_text(&ui).contains("\n40\n");
    let c_idle = !running(&ui);
    // The reply may still be streaming when we come back, so don't require idle on open.
    open_and_check_with(&ui, &log, "SessA", &["SessB", "SessC"], false).await;
    let finished = wait_until(120, || !running(&ui) && last_assistant(&ui).contains("DONE")).await;
    record(
        &log,
        "switch away mid-stream",
        streaming && c_clean && c_idle && finished && idle_clean(&ui).is_ok(),
        format!(
            "streaming before switch={streaming}; other chat free of the stream={c_clean}, idle={c_idle}; \
             back in the chat the reply finished with DONE={finished}; {:?}",
            idle_clean(&ui)
        ),
    );

    // ── The tab strip lists the chats this window has opened ─────────────────────
    let tabs: Vec<hermes_ui::TabItem> = st.get_tabs().iter().collect();
    let active: Vec<&hermes_ui::TabItem> = tabs.iter().filter(|t| t.active).collect();
    // `auto` mode: the strip itself only exists from the second chat on.
    let strip = ElementHandle::find_by_element_type_name(&ui, "TabStrip").count();
    record(
        &log,
        "tabs: every chat opened here has a tab, exactly one active",
        tabs.len() >= 3
            && active.len() == 1
            && active[0].id.as_str() == st.get_active_session().as_str()
            && strip == 1,
        format!(
            "{} tabs {:?}; active = {:?}; TabStrip drawn = {strip}",
            tabs.len(),
            tabs.iter().map(|t| (t.title.to_string(), t.active)).collect::<Vec<_>>(),
            active.first().map(|t| t.id.to_string())
        ),
    );

    // Selecting another tab switches the chat (the callbacks the strip's rows call).
    let other = tabs.iter().find(|t| !t.active).map(|t| t.id.to_string()).unwrap_or_default();
    st.invoke_select_tab(other.clone().into());
    let switched = wait_until(20, || st.get_active_session() == other).await;
    record(
        &log,
        "tabs: selecting a tab switches the chat",
        switched && !rows(&ui).is_empty(),
        format!("active={:?} rows={}", st.get_active_session(), rows(&ui).len()),
    );

    // Ctrl+W closes the active tab, lands on another one, and leaves the chat in the sidebar.
    let before_close = st.get_tabs().row_count();
    chord(&ui, &[Key::Control], "w");
    let closed = wait_until(20, || st.get_tabs().row_count() < before_close).await;
    let still_listed = st.get_sessions().iter().any(|s| s.id.as_str() == other.as_str());
    record(
        &log,
        "tabs: Ctrl+W closes the tab but not the chat",
        closed && still_listed && !st.get_active_session().is_empty() && idle_clean(&ui).is_ok(),
        format!(
            "tabs {before_close} → {}; chat still in the sidebar={still_listed}; active={:?}",
            st.get_tabs().row_count(),
            st.get_active_session()
        ),
    );

    // Ctrl+T: a new chat, drawn as the active draft tab.
    chord(&ui, &[Key::Control], "t");
    let draft = wait_until(10, || {
        st.get_active_session().is_empty() && st.get_tabs().iter().any(|t| t.active && t.title.as_str() == "New chat")
    })
    .await;
    record(
        &log,
        "tabs: Ctrl+T opens a new chat as the active tab",
        draft,
        format!(
            "{} tabs, titles {:?}",
            st.get_tabs().row_count(),
            st.get_tabs().iter().map(|t| t.title.to_string()).collect::<Vec<_>>()
        ),
    );
}

/// The private backend's home (what `scripts/live-test-backend.sh` created).
fn live_home() -> String {
    std::env::var("HERMES_LIVE_HOME").unwrap_or_else(|_| {
        format!(
            "{}/.cache/hermes-desktop-rs/live-home",
            std::env::var("HOME").unwrap_or_default()
        )
    })
}

/// The rows the settings panel is currently showing.
fn settings_rows(ui: &AppWindow) -> Vec<hermes_ui::SettingRowUI> {
    ui.global::<SettingsState>().get_rows().iter().collect()
}

fn row_value(ui: &AppWindow, id: &str) -> Option<String> {
    settings_rows(ui)
        .into_iter()
        .find(|r| r.id.as_str() == id)
        .map(|r| r.value.to_string())
}

/// The settings panel, end to end against the private backend: the registry renders, the
/// "Applies to" scope really switches profiles, an edit lands in that profile's config.yaml
/// (and only there), a device pref survives on disk, and a search hit jumps to its page.
///
/// Driven through the same Slint callbacks the widgets call (`invoke_edit` …) plus the real
/// Ctrl+, keybinding, so the controller wiring is exercised, not a mock.
async fn settings_section(ui: &AppWindow, log: &Log) {
    let s = ui.global::<SettingsState>();

    // ── Opens on the real keybinding and renders the registry page ───────────────
    chord(ui, &[Key::Control], ",");
    let opened = wait_until(20, || s.get_open() && !settings_rows(ui).is_empty()).await;
    let nav: Vec<hermes_ui::NavEntry> = s.get_nav().iter().collect();
    let groups = nav.iter().filter(|n| n.level == 0).count();
    let expected_groups = hermes_core::settings::GROUPS.len();
    let page_rows = settings_rows(ui);
    let nav_ok = nav.iter().any(|n| n.label.as_str() == "Appearance")
        && nav.iter().any(|n| n.level == 1 && n.page.as_str() == "typography");
    record(
        log,
        "settings: Ctrl+, opens the registry page",
        opened && groups == expected_groups && nav_ok && page_rows.iter().any(|r| r.id.as_str() == "display.language"),
        format!(
            "open={opened}; nav {} rows ({groups} groups, expected {expected_groups}, appearance expanded={nav_ok}); \
             Appearance › General shows {:?}",
            nav.len(),
            page_rows.iter().map(|r| r.id.to_string()).collect::<Vec<_>>()
        ),
    );

    // ── "Applies to": every profile is offered, the default is active ────────────
    let scopes: Vec<hermes_ui::ScopeChip> = s.get_scopes().iter().collect();
    let names: Vec<String> = scopes.iter().map(|c| c.name.to_string()).collect();
    record(
        log,
        "settings: profile pills list every profile",
        names.contains(&"default".to_owned())
            && names.contains(&"swe".to_owned())
            && names.contains(&"ea".to_owned())
            && scopes.iter().any(|c| c.name.as_str() == "default" && c.active),
        format!(
            "{names:?}, active={:?}",
            scopes
                .iter()
                .filter(|c| c.active)
                .map(|c| c.name.to_string())
                .collect::<Vec<_>>()
        ),
    );

    // ── Switching scope reads that profile's config ─────────────────────────────
    s.invoke_goto("appearance".into(), "general".into());
    sleep(200).await;
    let default_language = row_value(ui, "display.language");
    s.invoke_scope("swe".into());
    let switched = wait_until(20, || {
        s.get_scopes().iter().any(|c| c.name.as_str() == "swe" && c.active)
    })
    .await;
    sleep(600).await; // scoped config GET + re-render
    let swe_language = row_value(ui, "display.language");
    record(
        log,
        "settings: scope switch loads that profile's config",
        switched,
        format!("default={default_language:?} → swe={swe_language:?} (rows are the same registry page)"),
    );

    // ── An edit is saved into the scoped profile only ───────────────────────────
    let home = live_home();
    let swe_cfg_before = std::fs::read_to_string(format!("{home}/profiles/swe/config.yaml")).unwrap_or_default();
    let default_cfg_before = std::fs::read_to_string(format!("{home}/config.yaml")).unwrap_or_default();
    s.invoke_edit("display.language".into(), "de".into());
    let saved = wait_until(20, || row_value(ui, "display.language").as_deref() == Some("de")).await;
    sleep(300).await;
    let swe_cfg_after = std::fs::read_to_string(format!("{home}/profiles/swe/config.yaml")).unwrap_or_default();
    let default_cfg_after = std::fs::read_to_string(format!("{home}/config.yaml")).unwrap_or_default();
    record(
        log,
        "settings: a row saves into the scoped profile's config.yaml only",
        saved
            && swe_cfg_after.contains("language: de")
            && !swe_cfg_before.contains("language: de")
            && default_cfg_after == default_cfg_before,
        format!(
            "row shows de={saved}; swe config.yaml changed={}; default config.yaml untouched={}",
            swe_cfg_before != swe_cfg_after,
            default_cfg_after == default_cfg_before
        ),
    );

    // ── The saved value survives closing and reopening the panel ────────────────
    ui.set_settings_open(false);
    sleep(250).await;
    let closed = wait_until(5, || !s.get_open()).await;
    ui.set_settings_open(true);
    let reopened = wait_until(20, || s.get_open() && !settings_rows(ui).is_empty()).await;
    sleep(500).await;
    let after_reopen = row_value(ui, "display.language");
    record(
        log,
        "settings: a saved config value survives a reopen (fresh GET)",
        closed && reopened && after_reopen.as_deref() == Some("de"),
        format!("closed={closed}, reopened={reopened}, language after reopen={after_reopen:?}"),
    );

    // ── A device pref is written to prefs.json and read back from disk ──────────
    s.invoke_edit("hermes.desktop.intro-splash.v1".into(), "false".into());
    let toggled = wait_until(10, || {
        row_value(ui, "hermes.desktop.intro-splash.v1").as_deref() == Some("false")
    })
    .await;
    let prefs_file = std::env::var("HERMES_DESKTOP_PREFS").unwrap_or_default();
    let fresh = hermes_core::prefs::Prefs::load(&prefs_file);
    let on_disk = !fresh.get_bool("hermes.desktop.intro-splash.v1", true);
    record(
        log,
        "settings: a device pref persists to prefs.json",
        toggled && on_disk,
        format!("row shows false={toggled}; a fresh Prefs::load({prefs_file}) reads back false={on_disk}"),
    );

    // ── Search: a hit jumps to its page and highlights its row ──────────────────
    s.invoke_search("session list density".into());
    let hit_ready = wait_until(10, || s.get_hits().row_count() > 0).await;
    let hits: Vec<hermes_ui::SearchHit> = s.get_hits().iter().collect();
    let first = hits.first().cloned();
    let hit_ok = first
        .as_ref()
        .is_some_and(|h| h.page.as_str() == "window-layout" && !h.setting.is_empty());
    if let Some(h) = first.clone() {
        s.invoke_jump(h.group.clone(), h.page.clone(), h.setting.clone());
    }
    let jumped = wait_until(10, || {
        s.get_group().as_str() == "appearance"
            && s.get_page().as_str() == "window-layout"
            && s.get_query().is_empty()
            && settings_rows(ui)
                .iter()
                .any(|r| r.id == first.clone().map(|h| h.setting).unwrap_or_default() && r.highlight)
    })
    .await;
    record(
        log,
        "settings: a search hit jumps to the right page and highlights the row",
        hit_ready && hit_ok && jumped,
        format!(
            "hits={:?}; jumped to {}/{} with query cleared={}",
            hits.iter().map(|h| h.path.to_string()).collect::<Vec<_>>(),
            s.get_group(),
            s.get_page(),
            s.get_query().is_empty()
        ),
    );

    // ── Export → edit → Import → Reset, all against the scratch directory ───────
    // Back to the page that holds the Language row: the search jump moved us off it.
    s.invoke_goto("appearance".into(), "general".into());
    let back = wait_until(10, || {
        s.get_page().as_str() == "general" && row_value(ui, "display.language").is_some()
    })
    .await;
    let export_dir = std::path::PathBuf::from(std::env::var("HERMES_DESKTOP_EXPORT_DIR").unwrap_or_default());
    s.invoke_export_config();
    let exported_file = export_dir.join("hermes-config-swe.json");
    let exported = wait_until(10, || exported_file.exists()).await;
    let exported_language = std::fs::read_to_string(&exported_file)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| {
            v.get("display")
                .and_then(|d| d.get("language"))
                .and_then(|l| l.as_str())
                .map(str::to_owned)
        });
    record(
        log,
        "settings: Export writes the scoped profile's config",
        back && exported && exported_language.as_deref() == Some("de"),
        format!(
            "page back on General={back}; {} exists={exported}, display.language={exported_language:?}",
            exported_file.display()
        ),
    );

    let import_file = export_dir.join("imports").join("hermes-config-swe.json");
    let import_ok = if let Some(parent) = import_file.parent() {
        std::fs::create_dir_all(parent).is_ok()
            && std::fs::read_to_string(&exported_file)
                .ok()
                .map(|t| t.replace("\"language\": \"de\"", "\"language\": \"es\""))
                .map(|t| std::fs::write(&import_file, t).is_ok())
                .unwrap_or(false)
    } else {
        false
    };
    s.invoke_import_config();
    let imported = wait_until(15, || row_value(ui, "display.language").as_deref() == Some("es")).await;
    record(
        log,
        "settings: Import replaces the scoped config from a JSON file",
        import_ok && imported,
        format!("wrote {}={import_ok}; row shows es={imported}", import_file.display()),
    );

    // Reset is two clicks (the original confirms first); the first only arms it.
    s.invoke_reset_config();
    let armed_notice = wait_until(10, || s.get_notice().contains("Press reset again")).await;
    s.invoke_reset_config();
    let reset = wait_until(20, || {
        row_value(ui, "display.language").is_some_and(|v| v != "es" && v != "de")
    })
    .await;
    let swe_cfg = std::fs::read_to_string(format!("{home}/profiles/swe/config.yaml")).unwrap_or_default();
    record(
        log,
        "settings: Reset asks for a second click, then restores the backend defaults",
        armed_notice && reset && !swe_cfg.contains("language: es"),
        format!(
            "first click notice={armed_notice:?}; second click restored defaults={reset} (language now {:?})",
            row_value(ui, "display.language")
        ),
    );

    ui.set_settings_open(false);
    sleep(200).await;
}

/// The bottom status bar against the private backend: the health label, a real token gauge
/// built from the backend's own counters, the workspace branch, and the Ctrl+Shift+S toggle
/// (whose choice is a device pref, so it survives a restart).
async fn statusbar_section(ui: &AppWindow, log: &Log) {
    let st = ui.global::<AppState>();

    // A brand-new chat owns no counters: the bar must paint nothing, not a zero.
    st.invoke_new_chat();
    wait_until(5, || st.get_runtime_session().is_empty()).await;
    sleep(200).await;
    record(
        log,
        "status bar: a chat with no counters paints no reading",
        st.get_usage_label().is_empty() && st.get_branch_label().is_empty(),
        format!(
            "usage={:?} detail={:?} branch={:?}",
            st.get_usage_label(),
            st.get_usage_detail(),
            st.get_branch_label()
        ),
    );

    // Health follows the live connection, and the bar is on by default.
    record(
        log,
        "status bar: backend health tracks the connection",
        st.get_statusbar_open() && st.get_connection() == "open",
        format!("open={} connection={}", st.get_statusbar_open(), st.get_connection()),
    );

    // One real turn fills the gauge from the backend's counters.
    if !send_and_wait(ui, "Reply with exactly: status ok", 120).await {
        record(log, "status bar: turn for the token gauge", false, "no reply");
        return;
    }
    let filled = wait_until(10, || !st.get_usage_label().is_empty()).await;
    let sid = st.get_runtime_session().to_string();
    // An independent readback over a second connection: the bar must show those numbers.
    let reading = backend_request("session.usage", serde_json::json!({ "session_id": sid }))
        .await
        .ok()
        .and_then(|v| hermes_core::usage::from_value(&v));
    let label = st.get_usage_label().to_string();
    record(
        log,
        "status bar: the token gauge shows the backend's own reading",
        filled && reading.as_ref().is_some_and(|u| gauge_matches(&label, u)),
        format!(
            "ui={label:?} (detail {:?}) · backend {:?}",
            st.get_usage_detail(),
            reading.as_ref().map(hermes_core::usage::context_label)
        ),
    );
    screenshot(ui, "chrome-statusbar");

    // The bar follows the theme: capture the light palette too (DESIGN.md tokens).
    let theme = ui.global::<hermes_ui::Theme>();
    theme.set_mode(hermes_ui::ThemeMode::Light);
    sleep(300).await;
    screenshot(ui, "chrome-statusbar-light");
    theme.set_mode(hermes_ui::ThemeMode::Dark);
    sleep(200).await;

    // Ctrl+Shift+S hides the bar; the choice is a device pref, not session state.
    chord(ui, &[Key::Control, Key::Shift], "s");
    let hidden = wait_until(5, || !st.get_statusbar_open()).await;
    let prefs_file = std::env::var("HERMES_DESKTOP_PREFS").unwrap_or_default();
    let on_disk = !hermes_core::prefs::Prefs::load(&prefs_file).get_bool(hermes_core::prefs::STATUSBAR_VISIBLE, true);
    record(
        log,
        "status bar: Ctrl+Shift+S hides it and the choice persists",
        hidden && on_disk,
        format!("hidden={hidden}; prefs.json reads it hidden={on_disk}"),
    );
    chord(ui, &[Key::Control, Key::Shift], "s");
    let shown = wait_until(5, || st.get_statusbar_open()).await;
    record(
        log,
        "status bar: Ctrl+Shift+S brings it back",
        shown,
        format!("open={}", st.get_statusbar_open()),
    );

    // ── Sidebar surfaces: Sessions / Bots / Files ────────────────────────────────
    record(
        log,
        "sidebar: opens on the Sessions surface",
        st.get_sidebar_tab() == 0,
        format!("surface={}", st.get_sidebar_tab()),
    );
    let mut to_bots = false;
    if let Some(bots) = by_label(ui, "Bots") {
        bots.single_click(PointerEventButton::Left).await;
        to_bots = wait_until(5, || st.get_sidebar_tab() == 1).await;
    }
    record(
        log,
        "sidebar: clicking Bots switches the surface",
        to_bots,
        format!("surface={}", st.get_sidebar_tab()),
    );
    let mut back = false;
    if let Some(sessions) = by_label(ui, "Sessions") {
        sessions.single_click(PointerEventButton::Left).await;
        back = wait_until(5, || st.get_sidebar_tab() == 0).await;
    }
    record(
        log,
        "sidebar: clicking Sessions brings the chat list back",
        back && !st.get_sessions().iter().collect::<Vec<_>>().is_empty(),
        format!("surface={} · {} chats listed", st.get_sidebar_tab(), st.get_sessions().row_count()),
    );
}

/// The bar paints the context gauge when the backend knows one and the token total
/// otherwise; both forms are the same reading, so either matches.
fn gauge_matches(label: &str, u: &hermes_core::protocol::Usage) -> bool {
    label == hermes_core::usage::context_label(u)
        || label == format!("{} tok", hermes_core::usage::compact_number(u.total.unwrap_or(0)))
}

#[test]
#[ignore = "needs a running Hermes backend (HERMES_DESKTOP_PORT); see module docs"]
fn live_backend() {
    assert!(
        std::env::var_os("HERMES_DESKTOP_PORT").is_some(),
        "set HERMES_DESKTOP_PORT to a private test backend"
    );
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: false,
            threading: true,
            renderer_name: std::env::var_os("HERMES_LIVE_SCREENSHOTS").map(|_| "software".into()),
        },
    )))
    .unwrap();
    // The panel's own files (device prefs, config export/import) go to a scratch directory
    // so a live run never writes the user's prefs.json or ~/.local/share.
    let scratch = std::env::temp_dir().join(format!("hermes-live-settings-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).unwrap();
    unsafe {
        std::env::set_var("HERMES_DESKTOP_PREFS", scratch.join("prefs.json"));
        std::env::set_var("HERMES_DESKTOP_EXPORT_DIR", scratch.join("export"));
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let ui = AppWindow::new().unwrap();
    ui.window().set_size(slint::LogicalSize::new(1440.0, 900.0));
    hermes_ui::start(&ui, &rt);
    ui.show().unwrap();

    let log: Log = Rc::default();
    let (w, l) = (ui.as_weak(), log.clone());
    slint::spawn_local(async move {
        script(w, l).await;
        let _ = slint::quit_event_loop();
    })
    .unwrap();
    slint::run_event_loop().unwrap();
    let _ = ui.hide();
    rt.shutdown_timeout(Duration::from_secs(2));

    let log = log.borrow();
    let failed: Vec<_> = log
        .iter()
        .filter(|(_, ok, _)| !ok)
        .map(|(n, _, _)| n.as_str())
        .collect();
    println!("\n{} checks, {} failed", log.len(), failed.len());
    assert!(failed.is_empty(), "failed: {failed:?}");
}
