//! Live end-to-end test: the real UI (Slint testing backend, no window, no display) driven
//! against a real Hermes backend. Covers the PARITY items that need a live run:
//! composer keys (Enter / Shift+Enter / IME composition), Stop mid-stream (button and Esc),
//! model switching mid-session, the approval card (Deny / Allow once / Allow for session /
//! Always allow, by click and by keyboard), a long (600-message) conversation, and the
//! settings panel (registry nav, "Applies to" profile scope, saving, persistence, search).
//!
//! `HERMES_LIVE_ONLY=durable,rich_seeded,sidebar,composer,keys,stop,model,approval,long,sessions,settings,settings_pages,capabilities,models` runs a subset.
//! The `models` section covers the picker's catalog curation (visibility ticks, favourites,
//! hand-typed ids, group folding, Reset, and the Edit models dialog) — device-local state,
//! so it checks the scratch `prefs.json`; pass `HERMES_LIVE_SCREENSHOTS=<dir>` to also dump
//! picker/dialog snapshots as `.ppm` next to the run.
//! The `long` section
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
//! backend's own home). `HERMES_DESKTOP_STATE` points queue recovery at the same scratch
//! directory; the `durable` section spends two short model calls.
//!
//! The approval part runs `rm -r` on throwaway directories under
//! `$HERMES_LIVE_SCRATCH` (default `$TMPDIR/hermes-live-test`). "Always allow" writes
//! `command_allowlist` into the private backend's config.yaml
//! (`$HERMES_LIVE_HOME`, default `~/.cache/hermes-desktop-rs/live-home`, which
//! `scripts/live-test-backend.sh` regenerates on every start).

#[path = "live_backend/chat_display.rs"]
mod chat_display;
#[path = "live_backend/commands.rs"]
mod commands;
#[path = "live_backend/composer.rs"]
mod composer;
#[path = "live_backend/durable.rs"]
mod durable;
#[path = "live_backend/rich_seeded.rs"]
mod rich_seeded;
#[path = "live_backend/sidebar.rs"]
mod sidebar;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use hermes_ui::{AppState, AppWindow, CapState, SettingsState};
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

    if want("commands") {
        commands::section(&ui, &log).await;
    }
    if want("sidebar") {
        sidebar::section(&ui, &log).await;
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
    if want("models") {
        models_section(&ui, &log).await;
    }
    if want("approval") {
        approval_section(&ui, &log).await;
    }
    if want("rich") {
        rich_section(&ui, &log).await;
    }
    if want("rich_seeded") {
        rich_seeded::section(&ui, &log).await;
    }
    if want("composer") {
        composer::section(&ui, &log).await;
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
    if want("chat_display") {
        chat_display::section(&ui, &log).await;
    }
    if want("capabilities") {
        capabilities_section(&ui, &log).await;
    }
    if want("widgets") {
        settings_widgets_section(&ui, &log).await;
    }
    if want("settings_pages") {
        settings_pages_section(&ui, &log).await;
    }
    // Last: it adds two real chats to the sidebar, which would shift the seeded rows the
    // sessions/daily sections click.
    if want("durable") {
        durable::section(&ui, &log).await;
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

/// The prompt the `rich` section (and the README screenshot) sends.
const RICH_PROMPT: &str = "Reply with exactly these three parts and nothing else: \
    (1) a ```rust fenced block with a 3-line hello-world main function, \
    (2) a markdown table with columns Name and Count and two rows, \
    (3) a ```diff fenced block with a @@ hunk changing `let a = 1;` to `let a = 2;`.";

/// Assistant replies render as segments: code card (highlighted), GFM table, diff card.
async fn rich_section(ui: &AppWindow, log: &Log) {
    let (ui, log) = (ui.clone_strong(), log.clone());
    let done = send_and_wait(&ui, RICH_PROMPT, 180).await;
    let r = rows(&ui);
    let Some(last) = r.iter().rev().find(|r| r.kind == 1) else {
        record(
            &log,
            "rich reply rendered",
            false,
            format!("turn done = {done}, no assistant row"),
        );
        return;
    };
    let segs: Vec<hermes_ui::Seg> = last.segs.iter().collect();
    let kinds: Vec<i32> = segs.iter().map(|s| s.kind).collect();
    let code = segs.iter().find(|s| s.kind == 1);
    let table = segs.iter().find(|s| s.kind == 3);
    let diff = segs.iter().find(|s| s.kind == 2);
    record(
        &log,
        "rich reply → code, table and diff segments",
        done && code.is_some() && table.is_some() && diff.is_some(),
        format!("segment kinds = {kinds:?}"),
    );
    if let Some(c) = code {
        record(
            &log,
            "code card keeps the raw source for copy",
            c.raw.contains("fn main") && c.lines.row_count() >= 3,
            format!(
                "{} lines, raw starts {:?}",
                c.lines.row_count(),
                c.raw.chars().take(30).collect::<String>()
            ),
        );
    }
    if let Some(t) = table {
        record(
            &log,
            "table has a 2-column header and 2 body rows",
            t.header.cells.row_count() == 2 && t.rows.row_count() == 2,
            format!(
                "header {} cells, {} rows",
                t.header.cells.row_count(),
                t.rows.row_count()
            ),
        );
    }
    if let Some(d) = diff {
        let kinds: Vec<i32> = d.lines.iter().map(|l| l.kind).collect();
        record(
            &log,
            "diff card counts +1/−1 with line kinds",
            d.added >= 1 && d.removed >= 1 && kinds.contains(&1) && kinds.contains(&2),
            format!("+{} −{}, line kinds = {kinds:?}", d.added, d.removed),
        );
    }
    // The copy button is reachable by its accessible label.
    let copy = ElementHandle::find_by_accessible_label(&ui, "Copy code").count();
    record(
        &log,
        "code cards expose a Copy code button",
        copy >= 2,
        format!("{copy} Copy code buttons"),
    );
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

/// The picker's catalog curation (`models`): every provider the backend reports, the
/// visibility ticks, favourites, hand-typed ids, group folding, and Reset — plus the
/// menu's own footer actions driving the dialog. The catalog is the backend's; the
/// curation is device-local, so every write is checked in the scratch `prefs.json`
/// (`HERMES_DESKTOP_PREFS`) rather than in any real config.
async fn models_section(ui: &AppWindow, log: &Log) {
    use serde_json::json;
    let st = ui.global::<AppState>();
    let prefs_path = std::env::var("HERMES_DESKTOP_PREFS").unwrap_or_default();
    let prefs = || -> serde_json::Value {
        std::fs::read_to_string(&prefs_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| json!({}))
    };
    let stored_keys = |key: &str| -> Vec<String> {
        prefs()
            .get(key)
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };

    st.invoke_new_chat();
    // The menu's own "Refresh models": re-probes every provider.
    st.invoke_reload_models();
    let loaded = wait_until(25, || {
        st.get_models().iter().any(|m| !m.header) && st.get_model_edit_rows().iter().any(|r| !r.header)
    })
    .await;
    let providers: Vec<String> = st
        .get_model_edit_rows()
        .iter()
        .filter(|r| r.header)
        .map(|r| r.provider_name.to_string())
        .collect();
    let shown = st.get_models().iter().filter(|m| !m.header).count();
    record(
        log,
        "models: the picker lists the backend's providers and their models",
        loaded && !providers.is_empty() && shown > 0,
        format!(
            "{} providers {:?}, {shown} model rows, {} of {} models shown",
            providers.len(),
            providers,
            st.get_model_total_count() - st.get_model_hidden_count(),
            st.get_model_total_count()
        ),
    );

    // The list is the backend's, not the front end's: an independent RPC read agrees the
    // catalog has providers with models.
    let rpc = backend_request("model.options", json!({"include_unconfigured": true}))
        .await
        .unwrap_or_else(|_| json!({}));
    let rpc_models: usize = rpc["providers"]
        .as_array()
        .map(|ps| {
            ps.iter()
                .map(|p| p["models"].as_array().map(Vec::len).unwrap_or(0))
                .sum()
        })
        .unwrap_or(0);
    record(
        log,
        "models: the catalog is the backend's (model.options)",
        rpc_models > 0 && st.get_model_total_count() > 0,
        format!(
            "model.options reports {rpc_models} model ids over {} providers; the picker holds {}",
            rpc["providers"].as_array().map(Vec::len).unwrap_or(0),
            st.get_model_total_count()
        ),
    );

    let Some((provider, model)) = st
        .get_models()
        .iter()
        .find(|m| !m.header && !m.current)
        .map(|m| (m.provider.to_string(), m.model.to_string()))
    else {
        record(
            log,
            "models: an unchecked model is available to test",
            false,
            "no model row",
        );
        return;
    };
    let key = format!("{provider}::{model}");

    // Exactly the model the backend reports as current carries the tick.
    let cur_provider = rpc["provider"].as_str().unwrap_or_default();
    let cur_model = rpc["model"].as_str().unwrap_or_default();
    let ticks: Vec<(String, String)> = st
        .get_models()
        .iter()
        .filter(|m| m.current)
        .map(|m| (m.provider.to_string(), m.model.to_string()))
        .collect();
    let ticked = ticks
        .iter()
        .filter(|(p, m)| p == cur_provider && (m == cur_model || m.as_str() == cur_model.trim_end_matches("-fast")))
        .count();
    record(
        log,
        "models: the menu ticks the model the backend is on",
        cur_model.is_empty() || (ticks.len() == 1 && ticked == 1),
        format!("backend current = {cur_provider}/{cur_model}, ticks = {ticks:?}"),
    );
    if std::env::var_os("HERMES_LIVE_SCREENSHOTS").is_some() && !cur_model.is_empty() {
        ui.set_picker_open(true);
        // The field owns its text, exactly as typing does: set it, then tell the controller.
        st.set_model_query(cur_model.into());
        st.invoke_model_query_changed(cur_model.into());
        sleep(400).await;
        screenshot(ui, "model-picker-current");
        st.set_model_query("".into());
        st.invoke_model_query_changed("".into());
        ui.set_picker_open(false);
        sleep(200).await;
    }

    // Search: a query narrows the rows and reaches past the shortlist. The match folds
    // case and the `-_.` separators over id + provider, so the check does the same.
    let fold = |s: &str| -> String {
        s.to_lowercase()
            .chars()
            .map(|c| if matches!(c, '-' | '_' | '.') { ' ' } else { c })
            .collect()
    };
    let q = fold(&model);
    st.invoke_model_query_changed(model.as_str().into());
    let filtered = wait_until(5, || {
        let rows: Vec<_> = st.get_models().iter().filter(|m| !m.header).collect();
        !rows.is_empty()
            && rows
                .iter()
                .all(|m| fold(&format!("{} {} {}", m.model, m.provider_name, m.provider)).contains(&q))
    })
    .await;
    record(
        log,
        "models: searching the menu filters the catalog",
        filtered,
        format!("query={model:?}, rows={}", st.get_models().row_count()),
    );
    st.invoke_model_query_changed("".into());
    wait_until(5, || st.get_models().iter().any(|m| m.model == model.as_str())).await;

    // Uncheck → hidden + recorded; re-check → back.
    st.invoke_toggle_model(provider.as_str().into(), model.as_str().into());
    let gone = wait_until(5, || {
        !st.get_models()
            .iter()
            .any(|m| !m.header && m.provider == provider.as_str() && m.model == model.as_str())
    })
    .await;
    let after_uncheck = stored_keys("hermes.desktop.visible-models");
    record(
        log,
        "models: unchecking a model hides it and records the choice on this device",
        gone && !after_uncheck.iter().any(|k| k == &key),
        format!(
            "hidden={gone}, visible-models entries={}, hidden-count={}",
            after_uncheck.len(),
            st.get_model_hidden_count()
        ),
    );
    st.invoke_toggle_model(provider.as_str().into(), model.as_str().into());
    let back = wait_until(5, || {
        st.get_models()
            .iter()
            .any(|m| !m.header && m.provider == provider.as_str() && m.model == model.as_str())
    })
    .await;
    record(
        log,
        "models: re-checking brings the model back",
        back && stored_keys("hermes.desktop.visible-models").iter().any(|k| k == &key),
        format!("visible again={back}, stored={key}"),
    );

    // Star → the Favorites section, and the star is remembered in star order. The UI only
    // enqueues the intent, so every read waits for the controller to apply it.
    st.invoke_favorite_model(provider.as_str().into(), model.as_str().into());
    let pinned = wait_until(5, || {
        st.get_models()
            .iter()
            .next()
            .is_some_and(|m| m.header && m.provider_name == "Favorites")
            && st.get_models().iter().any(|m| m.favorite && m.model == model.as_str())
    })
    .await;
    let first = st
        .get_models()
        .iter()
        .next()
        .map(|m| (m.header, m.provider_name.to_string()));
    let fav_stored = stored_keys("hermes.desktop.favorite-models");
    record(
        log,
        "models: starring a model pins it to the Favorites section",
        pinned && fav_stored.iter().any(|k| k == &key),
        format!("first row={first:?}, pinned={pinned}, stored={fav_stored:?}"),
    );
    if std::env::var_os("HERMES_LIVE_SCREENSHOTS").is_some() {
        ui.set_picker_open(true);
        sleep(300).await;
        screenshot(ui, "model-picker-favorites");
        ui.set_picker_open(false);
    }
    st.invoke_favorite_model(provider.as_str().into(), model.as_str().into());
    let unpinned = wait_until(5, || {
        st.get_models()
            .iter()
            .next()
            .is_some_and(|m| m.provider_name != "Favorites")
            && !st.get_models().iter().any(|m| m.favorite)
    })
    .await;
    record(
        log,
        "models: unstarring drops the Favorites section",
        unpinned && stored_keys("hermes.desktop.favorite-models").is_empty(),
        format!("gone={unpinned}"),
    );

    // A hand-typed id joins its provider, shows at once, and can be forgotten again.
    let custom = format!("hermes-live-custom-{}", std::process::id());
    st.invoke_add_custom_model(provider.as_str().into(), custom.as_str().into());
    let added = wait_until(5, || {
        st.get_models()
            .iter()
            .any(|m| !m.header && m.custom && m.model == custom.as_str() && m.provider == provider.as_str())
    })
    .await;
    let custom_stored = prefs()["hermes.desktop.custom-models"].clone();
    record(
        log,
        "models: a hand-typed id is added under its provider and shows at once",
        added && custom_stored.to_string().contains(&custom),
        format!("shown={added}, stored={custom_stored}"),
    );
    st.invoke_remove_custom_model(provider.as_str().into(), custom.as_str().into());
    let forgot = wait_until(5, || !st.get_models().iter().any(|m| m.model == custom.as_str())).await;
    record(
        log,
        "models: removing a hand-typed id forgets it",
        forgot && !prefs()["hermes.desktop.custom-models"].to_string().contains(&custom),
        format!("forgotten={forgot}"),
    );

    // Folding a provider group, then its master switch in Edit models, then Reset.
    let before = st.get_models().row_count();
    st.invoke_toggle_provider_collapse(provider.as_str().into());
    let folded = wait_until(5, || {
        st.get_models()
            .iter()
            .any(|m| m.header && m.provider == provider.as_str() && m.collapsed)
            && st
                .get_models()
                .iter()
                .all(|m| m.header || m.provider != provider.as_str())
    })
    .await;
    record(
        log,
        "models: a provider group folds away and stays folded on this device",
        folded && st.get_models().row_count() < before,
        format!(
            "folded={folded}, rows {before} -> {}, stored={:?}",
            st.get_models().row_count(),
            stored_keys("hermes.desktop.collapsed-providers")
        ),
    );
    st.invoke_toggle_provider_collapse(provider.as_str().into());
    wait_until(5, || {
        st.get_models()
            .iter()
            .any(|m| !m.header && m.provider == provider.as_str())
    })
    .await;

    let header = st
        .get_model_edit_rows()
        .iter()
        .find(|r| r.header && r.provider == provider.as_str())
        .map(|r| (r.visible, r.count, r.total));
    record(
        log,
        "models: Edit models lists a provider's full row set with a master tick and counts",
        header.is_some_and(|(visible, count, total)| visible && count == total && total > 0),
        format!("header (visible, of, total) = {header:?}"),
    );
    st.invoke_toggle_provider_visible(provider.as_str().into());
    let all_off = wait_until(5, || {
        st.get_model_edit_rows()
            .iter()
            .find(|r| r.header && r.provider == provider.as_str())
            .is_some_and(|r| r.count == 0)
            && st
                .get_models()
                .iter()
                .all(|m| m.header || m.provider != provider.as_str())
    })
    .await;
    record(
        log,
        "models: a provider's master switch hides its whole group",
        all_off,
        format!(
            "rows in menu={}, hidden-count={}",
            st.get_models().iter().filter(|m| !m.header).count(),
            st.get_model_hidden_count()
        ),
    );
    st.invoke_reset_model_visibility();
    let restored = wait_until(5, || {
        st.get_model_edit_rows()
            .iter()
            .find(|r| r.header && r.provider == provider.as_str())
            .is_some_and(|r| r.count > 0)
            && prefs().get("hermes.desktop.visible-models").is_none()
    })
    .await;
    record(
        log,
        "models: Reset to defaults restores the curated list and clears the allow-list",
        restored,
        format!(
            "rows back={}, prefs key cleared={}",
            st.get_model_edit_rows()
                .iter()
                .find(|r| r.header && r.provider == provider.as_str())
                .map(|r| r.count)
                .unwrap_or(0),
            prefs().get("hermes.desktop.visible-models").is_none()
        ),
    );

    // The menu's footer actions, clicked for real.
    ui.set_picker_open(true);
    st.invoke_refresh_models();
    wait_until(10, || st.get_models().iter().any(|m| !m.header)).await;
    sleep(400).await;
    let clicked = click(ui, "Edit models…").await;
    let opened = wait_until(5, || st.get_model_editor_open()).await;
    record(
        log,
        "models: the menu's Edit models… action opens the dialog",
        clicked && opened && !ui.get_picker_open(),
        format!(
            "clicked={clicked}, open={}, menu closed={}",
            st.get_model_editor_open(),
            !ui.get_picker_open()
        ),
    );
    if std::env::var_os("HERMES_LIVE_SCREENSHOTS").is_some() {
        sleep(300).await;
        screenshot(ui, "model-editor");
    }
    let done = click(ui, "Done").await;
    let closed = wait_until(5, || !st.get_model_editor_open()).await;
    record(
        log,
        "models: Done closes the dialog",
        done && closed,
        format!("clicked={done}, closed={closed}"),
    );
    if std::env::var_os("HERMES_LIVE_SCREENSHOTS").is_some() {
        ui.set_picker_open(true);
        sleep(300).await;
        screenshot(ui, "model-picker");
        ui.set_picker_open(false);
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

/// Rows of the open page whose control is missing from the accessibility tree or shows a
/// different value there than the row model holds. Every row is instantiated (plain `for`
/// in a ScrollView, no virtualisation), so an absent control means it was not built.
/// Each control kind is matched on what it exposes: text inputs their value, toggles their
/// checked state, selects their current label, segmented controls their selected tab.
fn unrendered_controls(ui: &AppWindow, rows: &[hermes_ui::SettingRowUI]) -> Vec<String> {
    let labelled = |label: &str| ElementHandle::find_by_accessible_label(ui, label).collect::<Vec<_>>();
    let selected_label = |r: &hermes_ui::SettingRowUI| {
        usize::try_from(r.selected)
            .ok()
            .and_then(|i| r.option_labels.row_data(i))
            .map(|l| l.to_string())
    };
    rows.iter()
        .filter(|r| {
            let found = match r.kind.as_str() {
                "text" | "mono" | "number" | "list" => labelled(&r.label)
                    .iter()
                    .any(|e| e.accessible_value().as_deref() == Some(r.value.as_str())),
                "toggle" => labelled(&r.label)
                    .iter()
                    .any(|e| e.accessible_checked() == Some(r.value == "true")),
                "select" if r.option_labels.row_count() > 0 => {
                    let current = selected_label(r).unwrap_or_else(|| r.value.to_string());
                    labelled(&r.label)
                        .iter()
                        .any(|e| e.accessible_value().as_deref() == Some(current.as_str()))
                }
                "select" => labelled(&r.label)
                    .iter()
                    .any(|e| e.accessible_value().as_deref() == Some(r.value.as_str())),
                "segmented" => selected_label(r)
                    .is_some_and(|l| labelled(&l).iter().any(|e| e.accessible_item_selected() == Some(true))),
                "slider" => !labelled(&r.label).is_empty(),
                // Secret, widget, action and info rows are outside these six pages.
                _ => false,
            };
            !found
        })
        .map(|r| format!("{} ({})", r.id, r.kind))
        .collect()
}

/// The plain registry pages (Workspace, Safety, Browser, Memory & Context, Voice, Advanced),
/// against the default profile of the private backend. For every page, with the panel open
/// and no search query: the page model holds exactly the registry rows visible for the
/// loaded config, each config row holds the backend's value, and each row's control is in
/// the accessibility tree showing that value. Then one row per group is edited through the
/// panel, read back over REST (never the UI's own model), and put back, and the put-back is
/// confirmed over REST too. A row with no restorable baseline is never written.
async fn settings_pages_section(ui: &AppWindow, log: &Log) {
    use hermes_core::settings::{self, Store};

    let s = ui.global::<SettingsState>();
    let probe = match Probe::new() {
        Ok(p) => p,
        Err(e) => return record(log, "settings pages: REST read-back setup", false, e),
    };
    if !s.get_open() {
        chord(ui, &[Key::Control], ",");
    }
    if !wait_until(20, || s.get_open()).await {
        return record(
            log,
            "settings pages: Ctrl+, opens the panel",
            false,
            "panel stayed closed",
        );
    }
    // A leftover query shows search hits instead of the page. `search` only refreshes the
    // hits; the query itself is the field's two-way binding, so clear both like typing would.
    s.set_query("".into());
    s.invoke_search("".into());
    s.invoke_scope("default".into());
    sleep(600).await;
    let config = match probe.get("/api/config") {
        Ok(c) => c,
        Err(e) => return record(log, "settings pages: GET /api/config", false, e),
    };

    for group in ["workspace", "safety", "browser", "memory", "voice", "advanced"] {
        let pages = settings::group(group).map(|g| g.pages).unwrap_or_default();
        for page in pages {
            let expected: Vec<&settings::Setting> = settings::settings_for(group, page.id)
                .filter(|r| settings::visible(r, &config))
                .collect();
            s.invoke_goto(group.into(), page.id.into());
            let shown = wait_until(10, || {
                s.get_open()
                    && s.get_query().is_empty()
                    && s.get_group().as_str() == group
                    && s.get_page().as_str() == page.id
                    && settings_rows(ui).len() == expected.len()
            })
            .await;
            let rows = settings_rows(ui);
            let unrendered = unrendered_controls(ui, &rows);
            let ids: Vec<String> = rows.iter().map(|r| r.id.to_string()).collect();
            let want_ids: Vec<&str> = expected.iter().map(|r| r.id).collect();
            let mismatched: Vec<String> = expected
                .iter()
                .filter(|r| matches!(r.store, Store::Config(_)))
                .filter_map(|r| {
                    let want = settings::row_value(r, &config, &serde_json::Map::new());
                    let got = rows.iter().find(|x| x.id.as_str() == r.id)?.value.to_string();
                    (got != want).then(|| format!("{}: panel {got:?} vs backend {want:?}", r.id))
                })
                .collect();
            record(
                log,
                &format!("settings pages: {group} › {} renders its registry rows", page.id),
                shown && ids == want_ids && mismatched.is_empty() && unrendered.is_empty(),
                format!(
                    "{} rows {ids:?} (expected {}); value mismatches {mismatched:?}; \
                     controls missing from the accessibility tree {unrendered:?}",
                    ids.len(),
                    want_ids.len()
                ),
            );
        }
    }

    // One write per group: a numeric or toggle row that changes no safety behaviour of the
    // private backend (approvals.mode stays manual).
    let writes = [
        ("workspace", "files", "file_read_max_chars", "123457", "123458"),
        ("safety", "approvals", "approvals.timeout", "61", "62"),
        (
            "browser",
            "network",
            "browser.auto_local_for_private_urls",
            "true",
            "false",
        ),
        ("memory", "persistent", "memory.memory_char_limit", "2201", "2202"),
        ("voice", "conversation", "voice.max_recording_seconds", "121", "122"),
        ("advanced", "runtime", "agent.max_turns", "91", "92"),
    ];
    for (group, page, key, a, b) in writes {
        s.invoke_goto(group.into(), page.into());
        let name = format!("settings pages: {group} › {page} saves {key} (REST read-back)");
        let backend_value = || {
            probe
                .get("/api/config")
                .ok()
                .and_then(|c| settings::get(&c, key).map(settings::value_to_string))
        };
        let _ = wait_until(10, || row_value(ui, key).is_some()).await;
        let before = row_value(ui, key).unwrap_or_default();
        // An absent key cannot be put back through the panel (it would leave a value
        // behind), so the row is not written at all.
        if before.is_empty() || backend_value().as_deref() != Some(before.as_str()) {
            record(
                log,
                &name,
                false,
                format!(
                    "no restorable baseline: panel {before:?}, backend {:?}; not written",
                    backend_value()
                ),
            );
            continue;
        }
        let new = if before == a { b } else { a };
        s.invoke_edit(key.into(), new.into());
        let shown = wait_until(10, || row_value(ui, key).as_deref() == Some(new)).await;
        sleep(300).await;
        let stored = backend_value();
        s.invoke_edit(key.into(), before.clone().into());
        let _ = wait_until(10, || row_value(ui, key).as_deref() == Some(before.as_str())).await;
        sleep(300).await;
        let restored = backend_value();
        record(
            log,
            &name,
            shown && stored.as_deref() == Some(new) && restored.as_deref() == Some(before.as_str()),
            format!(
                "{before:?} → {new:?}: panel={shown}, backend config={stored:?}, backend after put-back={restored:?}"
            ),
        );
    }

    ui.set_settings_open(false);
    sleep(200).await;
}

/// An independent REST read/write against the private backend (same endpoint and token
/// discovery as the app), to verify what the Capabilities screen persisted.
async fn backend_rest(method: &str, path: &str, body: Option<serde_json::Value>) -> Result<serde_json::Value, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let (method, path) = (method.to_owned(), path.to_owned());
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt.block_on(async {
            let backend = hermes_core::backend::discover_or_spawn(None)
                .await
                .map_err(|e| e.to_string())?;
            let e = &backend.endpoint;
            hermes_core::http::Rest::new(e.host.clone(), e.port, e.token.clone())
                .send(&method, &path, body.as_ref())
                .await
                .map_err(|e| e.to_string())
        });
        let _ = tx.send(result);
    });
    rx.await.map_err(|e| e.to_string())?
}

fn cap_items(ui: &AppWindow) -> Vec<hermes_ui::CapItem> {
    ui.global::<CapState>().get_items().iter().collect()
}

fn cap_has(ui: &AppWindow, id: &str) -> bool {
    cap_items(ui).iter().any(|i| i.id.as_str() == id)
}

/// Set a LineEdit/TextEdit by its accessible label (as typing would).
fn set_field(ui: &AppWindow, label: &str, text: &str) -> bool {
    by_label(ui, label).map(|e| e.set_accessible_value(text)).is_some()
}

/// Click a Capabilities tab. Tab names also label sidebar rows ("Scheduled") and hidden
/// settings rows, so only an element with the Tab role counts.
async fn cap_tab(ui: &AppWindow, label: &str, index: i32) -> bool {
    let s = ui.global::<CapState>();
    sleep(150).await;
    let Some(tab) = ElementHandle::find_by_accessible_label(ui, label)
        .find(|e| e.accessible_role() == Some(i_slint_backend_testing::AccessibleRole::Tab))
    else {
        return false;
    };
    tab.single_click(PointerEventButton::Left).await;
    // The tab index flips on the click itself; the controller publishes the tab's form
    // (and resets its fields) afterwards. Typing before that would land in the old form.
    let tab_kind = hermes_core::capabilities::Tab::from_index(index);
    let button = hermes_core::capabilities::form(tab_kind).map_or("", |f| f.button);
    let first = hermes_core::capabilities::form(tab_kind).map_or("", |f| f.fields[0]);
    let form_ready = wait_until(20, || {
        s.get_tab() == index
            && s.get_form_button().as_str() == button
            && s.get_form_labels().row_data(0).is_some_and(|l| l.as_str() == first)
    })
    .await;
    sleep(300).await;
    form_ready && wait_until(20, || !s.get_loading()).await && !s.get_notice_error()
}

fn json_list<'a>(v: &'a serde_json::Value, key: &str) -> Vec<&'a serde_json::Value> {
    v.get(key)
        .and_then(|x| x.as_array())
        .or_else(|| v.as_array())
        .map(|a| a.iter().collect())
        .unwrap_or_default()
}

fn find_named<'a>(list: &[&'a serde_json::Value], name: &str) -> Option<&'a serde_json::Value> {
    list.iter()
        .copied()
        .find(|x| x.get("name").and_then(|n| n.as_str()) == Some(name))
}

async fn capabilities_section(ui: &AppWindow, log: &Log) {
    let s = ui.global::<CapState>();
    let tag = std::process::id();
    let home = live_home();

    // ── Sidebar "Scheduled" opens the screen on the Scheduled tab ────────────────
    let opened =
        click(ui, "Scheduled").await && wait_until(20, || s.get_open() && s.get_tab() == 5 && !s.get_loading()).await;
    record(
        log,
        "capabilities: sidebar Scheduled opens the Scheduled tab",
        opened && !s.get_notice_error(),
        format!("open={} tab={} notice={:?}", s.get_open(), s.get_tab(), s.get_notice()),
    );

    // ── Profiles: list, create, edit SOUL.md, rename, delete (two clicks) ────────
    let listed = cap_tab(ui, "Profiles", 0).await
        && wait_until(10, || cap_has(ui, "default") && cap_has(ui, "swe") && cap_has(ui, "ea")).await;
    record(
        log,
        "capabilities: Profiles lists every profile",
        listed,
        format!(
            "{:?}",
            cap_items(ui).iter().map(|i| i.id.to_string()).collect::<Vec<_>>()
        ),
    );

    let name = format!("rscap{tag}");
    let renamed = format!("rscapr{tag}");
    set_field(ui, "Profile name", &name);
    click(ui, "Create profile").await;
    let created = wait_until(30, || cap_has(ui, &name)).await;
    let rest = backend_rest("GET", "/api/profiles", None).await.unwrap_or_default();
    let on_backend = find_named(&json_list(&rest, "profiles"), &name).is_some();
    let real_wrapper = std::path::Path::new(&std::env::var("HOME").unwrap_or_default())
        .join(".local/bin")
        .join(&name)
        .exists();
    record(
        log,
        "capabilities: Create profile persists on the backend",
        created && on_backend && !real_wrapper,
        format!(
            "row shown={created}; GET /api/profiles has it={on_backend}; wrapper in real ~/.local/bin={real_wrapper}"
        ),
    );

    click(ui, &format!("Soul {name}")).await;
    let soul_open = wait_until(10, || s.get_detail_open() && !s.get_detail_single_line()).await;
    set_field(ui, "Detail text", "Throwaway soul written by the live test.");
    click(ui, "Save").await;
    let soul_saved = wait_until(10, || !s.get_detail_open()).await;
    let soul = backend_rest("GET", &format!("/api/profiles/{name}/soul"), None)
        .await
        .unwrap_or_default();
    let soul_ok = soul.get("content").and_then(|c| c.as_str()) == Some("Throwaway soul written by the live test.");
    record(
        log,
        "capabilities: SOUL.md edit saves to the profile",
        soul_open && soul_saved && soul_ok,
        format!("editor opened={soul_open}; closed after save={soul_saved}; GET soul matches={soul_ok}"),
    );

    click(ui, &format!("Rename {name}")).await;
    let rename_open = wait_until(10, || s.get_detail_open() && s.get_detail_single_line()).await;
    set_field(ui, "Detail text", &renamed);
    click(ui, "Save").await;
    let rename_ok = wait_until(30, || cap_has(ui, &renamed) && !cap_has(ui, &name)).await;
    record(
        log,
        "capabilities: Rename profile",
        rename_open && rename_ok,
        format!("rename field opened={rename_open}; list shows {renamed} only={rename_ok}"),
    );

    click(ui, &format!("Delete {renamed}")).await;
    let armed = wait_until(5, || s.get_armed().as_str() == format!("{renamed}|delete")).await;
    let still_there = cap_has(ui, &renamed);
    click(ui, &format!("Delete {renamed}")).await;
    let deleted = wait_until(30, || !cap_has(ui, &renamed)).await;
    let rest = backend_rest("GET", "/api/profiles", None).await.unwrap_or_default();
    let gone = find_named(&json_list(&rest, "profiles"), &renamed).is_none();
    record(
        log,
        "capabilities: Delete profile needs a second click",
        armed && still_there && deleted && gone,
        format!(
            "first click armed={armed} (row kept={still_there}); second click removed row={deleted}; backend gone={gone}"
        ),
    );

    // ── Skills: create + view, switch, then the same in the swe profile ──────────
    let loaded = cap_tab(ui, "Skills", 1).await;
    let skill_name = format!("rs-cap-skill-{tag}");
    let enabled_in = |v: &serde_json::Value, name: &str| {
        find_named(&json_list(v, "skills"), name)
            .and_then(|x| x.get("enabled"))
            .and_then(|e| e.as_bool())
    };
    set_field(ui, "Skill name", &skill_name);
    set_field(ui, "Description", "Throwaway skill created by the live test.");
    click(ui, "Create skill").await;
    let made = wait_until(20, || cap_has(ui, &skill_name)).await;
    // A real skill list is longer than the window; the filter brings the row into view.
    let typed = set_field(ui, "Filter capabilities", &skill_name);
    s.invoke_filter(skill_name.as_str().into());
    let filtered = wait_until(10, || cap_items(ui).len() == 1 && cap_has(ui, &skill_name)).await;
    click(ui, &format!("View {skill_name}")).await;
    let viewed = wait_until(10, || {
        s.get_detail_open() && s.get_detail_text().contains(&format!("name: {skill_name}"))
    })
    .await;
    click(ui, "Back").await;
    record(
        log,
        "capabilities: Create skill, filter to it, then view its SKILL.md",
        loaded && made && typed && filtered && viewed,
        format!(
            "tab loaded={loaded}; row shown={made}; filter left only it={filtered}; \
             SKILL.md opened with frontmatter={viewed}; notice={:?}",
            s.get_notice()
        ),
    );

    click(ui, &format!("Enable {skill_name}")).await;
    let off = wait_until(10, || {
        cap_items(ui).iter().any(|i| i.id.as_str() == skill_name && !i.on)
    })
    .await;
    let rest_off = enabled_in(
        &backend_rest("GET", "/api/skills", None).await.unwrap_or_default(),
        &skill_name,
    );
    click(ui, &format!("Enable {skill_name}")).await;
    let on = wait_until(10, || cap_items(ui).iter().any(|i| i.id.as_str() == skill_name && i.on)).await;
    let rest_on = enabled_in(
        &backend_rest("GET", "/api/skills", None).await.unwrap_or_default(),
        &skill_name,
    );
    record(
        log,
        "capabilities: skill switch writes the backend and reads back",
        off && rest_off == Some(false) && on && rest_on == Some(true),
        format!("off row={off} backend={rest_off:?}; on row={on} backend={rest_on:?}"),
    );

    // The profile pills scope create and switch to that profile only.
    click(ui, "Use profile swe").await;
    let scoped = wait_until(20, || s.get_scope().as_str() == "swe" && !s.get_loading()).await;
    let absent_in_swe = !cap_has(ui, &skill_name);
    set_field(ui, "Skill name", &skill_name);
    set_field(ui, "Description", "Throwaway skill created by the live test.");
    click(ui, "Create skill").await;
    let made_swe = wait_until(20, || cap_has(ui, &skill_name)).await;
    click(ui, &format!("Enable {skill_name}")).await;
    let swe_off = wait_until(10, || {
        cap_items(ui).iter().any(|i| i.id.as_str() == skill_name && !i.on)
    })
    .await;
    let swe_rest = enabled_in(
        &backend_rest("GET", "/api/skills?profile=swe", None)
            .await
            .unwrap_or_default(),
        &skill_name,
    );
    let default_rest = enabled_in(
        &backend_rest("GET", "/api/skills", None).await.unwrap_or_default(),
        &skill_name,
    );
    click(ui, "Use profile default").await;
    let back_default = wait_until(20, || s.get_scope().is_empty() && !s.get_loading()).await;
    record(
        log,
        "capabilities: profile pills scope skill create and switch to that profile",
        scoped
            && absent_in_swe
            && made_swe
            && swe_off
            && swe_rest == Some(false)
            && default_rest == Some(true)
            && back_default,
        format!(
            "scope swe={scoped}; absent before={absent_in_swe}; created in swe={made_swe}; off in swe row={swe_off} \
             backend swe={swe_rest:?} default={default_rest:?}; back to default={back_default}"
        ),
    );

    // ── Tools: toggle a toolset that has no install step ─────────────────────────
    const INSTALLS: [&str; 8] = [
        "tts",
        "stt",
        "web",
        "image_gen",
        "video_gen",
        "x_search",
        "browser",
        "computer_use",
    ];
    let tools_loaded = cap_tab(ui, "Tools", 2).await && wait_until(10, || !cap_items(ui).is_empty()).await;
    let ts = cap_items(ui)
        .into_iter()
        .find(|i| i.has_toggle && i.on && !INSTALLS.contains(&i.id.as_str()));
    if let Some(t) = ts {
        let tid = t.id.to_string();
        let enabled = |v: &serde_json::Value| {
            find_named(&json_list(v, "toolsets"), &tid)
                .and_then(|x| x.get("enabled"))
                .and_then(|e| e.as_bool())
        };
        click(ui, &format!("Enable {}", t.title)).await;
        let off = wait_until(10, || cap_items(ui).iter().any(|i| i.id == t.id && !i.on)).await;
        let rest_off = enabled(
            &backend_rest("GET", "/api/tools/toolsets", None)
                .await
                .unwrap_or_default(),
        );
        click(ui, &format!("Enable {}", t.title)).await;
        let on = wait_until(10, || cap_items(ui).iter().any(|i| i.id == t.id && i.on)).await;
        let rest_on = enabled(
            &backend_rest("GET", "/api/tools/toolsets", None)
                .await
                .unwrap_or_default(),
        );
        record(
            log,
            "capabilities: toolset switch writes the backend and reads back",
            tools_loaded && off && rest_off == Some(false) && on && rest_on == Some(true),
            format!("{tid}: off row={off} backend={rest_off:?}; on row={on} backend={rest_on:?}"),
        );
    } else {
        record(
            log,
            "capabilities: toolset switch writes the backend and reads back",
            false,
            "no safe enabled toolset",
        );
    }

    // ── MCP: add (no test call: that would spawn the server), disable, remove ────
    let mcp = format!("rscapmcp{tag}");
    let mcp_loaded = cap_tab(ui, "MCP", 3).await;
    set_field(ui, "Server name", &mcp);
    set_field(ui, "Command or URL", "echo");
    set_field(ui, "Arguments", "live-test");
    click(ui, "Add server").await;
    let added = wait_until(20, || cap_has(ui, &mcp)).await;
    let servers = backend_rest("GET", "/api/mcp/servers", None).await.unwrap_or_default();
    let cfg = find_named(&json_list(&servers, "servers"), &mcp).cloned();
    let shape_ok = cfg.as_ref().is_some_and(|c| {
        c.get("command").and_then(|x| x.as_str()) == Some("echo")
            && c.get("args") == Some(&serde_json::json!(["live-test"]))
    });
    click(ui, &format!("Enable {mcp}")).await;
    let disabled = wait_until(10, || cap_items(ui).iter().any(|i| i.id.as_str() == mcp && !i.on)).await;
    let servers = backend_rest("GET", "/api/mcp/servers", None).await.unwrap_or_default();
    let rest_disabled = find_named(&json_list(&servers, "servers"), &mcp)
        .and_then(|c| c.get("enabled"))
        .and_then(|e| e.as_bool());
    click(ui, &format!("Remove {mcp}")).await;
    click(ui, &format!("Remove {mcp}")).await;
    let removed = wait_until(20, || !cap_has(ui, &mcp)).await;
    let servers = backend_rest("GET", "/api/mcp/servers", None).await.unwrap_or_default();
    let rest_gone = find_named(&json_list(&servers, "servers"), &mcp).is_none();
    record(
        log,
        "capabilities: MCP add → disable → remove round-trips the backend",
        mcp_loaded && added && shape_ok && disabled && rest_disabled == Some(false) && removed && rest_gone,
        format!(
            "added={added} saved as echo [live-test]={shape_ok}; disabled row={disabled} backend={rest_disabled:?}; removed={removed} backend gone={rest_gone}"
        ),
    );

    // ── Memory: reset a fixture MEMORY.md (two clicks); pause/resume the curator ─
    let mem_dir = std::path::Path::new(&home).join("memories");
    let fixture = mem_dir.join("MEMORY.md");
    let wrote = std::fs::create_dir_all(&mem_dir).is_ok() && std::fs::write(&fixture, "live-test fixture\n").is_ok();
    let mem_loaded = cap_tab(ui, "Memory", 4).await;
    let sized = wait_until(10, || {
        cap_items(ui)
            .iter()
            .any(|i| i.id.as_str() == "file:memory" && !i.subtitle.ends_with(" 0 B"))
    })
    .await;
    click(ui, "Reset MEMORY.md").await;
    let kept_after_first = fixture.exists();
    click(ui, "Reset MEMORY.md").await;
    let reset = wait_until(10, || !fixture.exists()).await;
    record(
        log,
        "capabilities: Reset MEMORY.md needs a second click and deletes the file",
        wrote && mem_loaded && sized && kept_after_first && reset,
        format!(
            "fixture written={wrote}; size shown={sized}; kept after first click={kept_after_first}; deleted={reset}"
        ),
    );

    let paused = |v: &serde_json::Value| v.get("paused").and_then(|p| p.as_bool());
    let before = paused(&backend_rest("GET", "/api/curator", None).await.unwrap_or_default());
    if before.is_some() && cap_has(ui, "curator") {
        click(ui, "Enable Skill curator").await;
        let flipped = wait_until(10, || {
            cap_items(ui)
                .iter()
                .any(|i| i.id.as_str() == "curator" && i.on == before.unwrap_or(false))
        })
        .await;
        let mid = paused(&backend_rest("GET", "/api/curator", None).await.unwrap_or_default());
        click(ui, "Enable Skill curator").await;
        let restored = wait_until(10, || {
            cap_items(ui)
                .iter()
                .any(|i| i.id.as_str() == "curator" && i.on != before.unwrap_or(false))
        })
        .await;
        let after = paused(&backend_rest("GET", "/api/curator", None).await.unwrap_or_default());
        record(
            log,
            "capabilities: curator switch pauses and resumes",
            flipped && mid == before.map(|b| !b) && restored && after == before,
            format!("paused before={before:?} mid={mid:?} after={after:?}"),
        );
    } else {
        record(
            log,
            "capabilities: curator switch pauses and resumes",
            false,
            format!("curator status {before:?}"),
        );
    }

    // ── Scheduled: create (yearly, never due during the test), pause, edit, delete ─
    let job = format!("rs-cap-job-{tag}");
    let cron_loaded = cap_tab(ui, "Scheduled", 5).await;
    set_field(ui, "Job name", &job);
    set_field(ui, "Schedule", "0 3 1 1 *");
    set_field(ui, "Prompt", "Say hi.");
    click(ui, "Schedule job").await;
    let made = wait_until(20, || cap_items(ui).iter().any(|i| i.title.as_str() == job)).await;
    let jobs = backend_rest("GET", "/api/cron/jobs?profile=all", None)
        .await
        .unwrap_or_default();
    let rest_job = find_named(&json_list(&jobs, "jobs"), &job).cloned();
    let id = rest_job
        .as_ref()
        .and_then(|j| j.get("id"))
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_owned();
    record(
        log,
        "capabilities: Schedule job creates a cron job",
        cron_loaded && made && !id.is_empty(),
        format!("row shown={made}; backend id={id:?}"),
    );

    click(ui, &format!("Enable {job}")).await;
    let paused_row = wait_until(10, || cap_items(ui).iter().any(|i| i.title.as_str() == job && !i.on)).await;
    let state = backend_rest("GET", &format!("/api/cron/jobs/{id}"), None)
        .await
        .unwrap_or_default()
        .get("state")
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_owned();
    record(
        log,
        "capabilities: job switch pauses it",
        paused_row && state == "paused",
        format!("row off={paused_row}; backend state={state:?}"),
    );

    click(ui, &format!("Enable {job}")).await;
    let resumed_row = wait_until(10, || cap_items(ui).iter().any(|i| i.title.as_str() == job && i.on)).await;
    let state = backend_rest("GET", &format!("/api/cron/jobs/{id}"), None)
        .await
        .unwrap_or_default()
        .get("state")
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_owned();
    record(
        log,
        "capabilities: job switch resumes it",
        resumed_row && state == "scheduled",
        format!("row on={resumed_row}; backend state={state:?}"),
    );

    click(ui, &format!("Prompt {job}")).await;
    let prompt_open = wait_until(10, || s.get_detail_open() && s.get_detail_text().as_str() == "Say hi.").await;
    set_field(ui, "Detail text", "Say hello.");
    click(ui, "Save").await;
    let saved = wait_until(10, || !s.get_detail_open()).await;
    let prompt = backend_rest("GET", &format!("/api/cron/jobs/{id}"), None)
        .await
        .unwrap_or_default()
        .get("prompt")
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_owned();
    record(
        log,
        "capabilities: job prompt edit saves",
        prompt_open && saved && prompt == "Say hello.",
        format!("editor showed the prompt={prompt_open}; saved={saved}; backend prompt={prompt:?}"),
    );

    // "Run now" spends a model call: check only that one click arms it and runs nothing.
    click(ui, &format!("Run now {job}")).await;
    let run_armed = wait_until(5, || s.get_notice().contains("uses your model")).await;
    sleep(500).await;
    let last_run = backend_rest("GET", &format!("/api/cron/jobs/{id}"), None)
        .await
        .unwrap_or_default()
        .get("last_run_at")
        .cloned()
        .unwrap_or_default();
    record(
        log,
        "capabilities: one Run now click only asks for confirmation",
        run_armed && last_run.is_null(),
        format!("notice warns about model use={run_armed}; last_run_at={last_run}"),
    );

    click(ui, &format!("Delete {job}")).await;
    click(ui, &format!("Delete {job}")).await;
    let job_gone = wait_until(20, || !cap_items(ui).iter().any(|i| i.title.as_str() == job)).await;
    let jobs = backend_rest("GET", "/api/cron/jobs?profile=all", None)
        .await
        .unwrap_or_default();
    let rest_gone = find_named(&json_list(&jobs, "jobs"), &job).is_none();
    record(
        log,
        "capabilities: Delete job (two clicks)",
        !id.is_empty() && job_gone && rest_gone,
        format!(
            "job existed={}; row gone={job_gone}; backend gone={rest_gone}",
            !id.is_empty()
        ),
    );

    if std::env::var_os("HERMES_LIVE_SCREENSHOTS").is_some() {
        cap_tab(ui, "Skills", 1).await;
        sleep(400).await;
        let out = format!("{}/capabilities-skills.ppm", tmpdir());
        if let Ok(buf) = ui.window().take_snapshot() {
            let mut ppm = format!("P6\n{} {}\n255\n", buf.width(), buf.height()).into_bytes();
            for px in buf.as_slice() {
                ppm.extend_from_slice(&[px.r, px.g, px.b]);
            }
            let _ = std::fs::write(&out, ppm);
            println!("[SHOT] {out}");
        }
    }

    press(ui, Key::Escape);
    let closed = wait_until(5, || !s.get_open()).await;
    record(
        log,
        "capabilities: Esc closes the screen",
        closed,
        format!("open={}", s.get_open()),
    );
}

/// Independent REST read-backs to the private backend. The test body runs on the Slint
/// event loop, which is not a tokio context, and `hermes_core`'s HTTP helpers use
/// `tokio::time::timeout` internally — so the probe owns a current-thread runtime and
/// blocks on it for each call (milliseconds, local socket).
struct Probe {
    rt: tokio::runtime::Runtime,
    rest: hermes_core::http::Rest,
}

impl Probe {
    fn new() -> Result<Self, String> {
        let port: u16 = std::env::var("HERMES_DESKTOP_PORT")
            .map_err(|_| "HERMES_DESKTOP_PORT not set".to_owned())?
            .parse()
            .map_err(|e| format!("bad HERMES_DESKTOP_PORT: {e}"))?;
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let token = rt
            .block_on(hermes_core::backend::fetch_token("127.0.0.1", port))
            .map_err(|e| e.to_string())?;
        Ok(Self {
            rt,
            rest: hermes_core::http::Rest::new("127.0.0.1", port, token),
        })
    }

    fn get(&self, path: &str) -> Result<serde_json::Value, String> {
        self.rt.block_on(self.rest.get(path)).map_err(|e| e.to_string())
    }

    /// The session mutation routes answer 405 to PUT — they need PATCH.
    fn patch(&self, path: &str, body: &serde_json::Value) -> Result<serde_json::Value, String> {
        self.rt.block_on(self.rest.patch(path, body)).map_err(|e| e.to_string())
    }
}

/// The four settings widget areas, end to end against the private backend. Every page whose
/// row used to render "Not built yet" must load its real state, and each area must prove one
/// write lands by an independent REST read-back (never the UI's own model).
async fn settings_widgets_section(ui: &AppWindow, log: &Log) {
    let s = ui.global::<SettingsState>();
    let probe = match Probe::new() {
        Ok(p) => p,
        Err(e) => return record(log, "widgets: REST read-back setup", false, e),
    };
    if !wait_until(20, || s.get_open()).await {
        chord(ui, &[Key::Control], ",");
        let _ = wait_until(20, || s.get_open()).await;
    }

    // ── appearance/theme: the palette grid ──────────────────────────────────────
    s.invoke_goto("appearance".into(), "theme".into());
    let a = ui.global::<hermes_ui::AppearanceState>();
    let palettes = wait_until(20, || a.get_palettes().row_count() > 0).await;
    let backend_themes = probe.get("/api/dashboard/themes").ok();
    let names: Vec<String> = backend_themes
        .as_ref()
        .and_then(|v| v.get("themes"))
        .and_then(|t| t.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|t| t.get("name").and_then(|n| n.as_str()).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let active = backend_themes
        .as_ref()
        .and_then(|v| v.get("active"))
        .and_then(|a| a.as_str())
        .unwrap_or_default()
        .to_owned();
    let grid_shows = a.get_palettes().row_count();
    record(
        log,
        "widgets: Appearance › Theme lists the backend palettes",
        palettes && grid_shows == names.len() && !names.is_empty(),
        format!(
            "grid rows={grid_shows}, backend themes={names:?} (active {active:?}), palette-error={:?}",
            a.get_palette_error()
        ),
    );

    // Pick a palette that is not active and verify the backend switched.
    if let Some(pick) = names.iter().find(|n| n.as_str() != active) {
        s.invoke_action(format!("appearance:palette:{pick}").into());
        let mut switched = false;
        for _ in 0..67 {
            let now_active = probe
                .get("/api/dashboard/themes")
                .ok()
                .and_then(|v| v.get("active").and_then(|a| a.as_str()).map(str::to_owned));
            if now_active.as_deref() == Some(pick.as_str()) {
                switched = true;
                break;
            }
            sleep(300).await;
        }
        // Restore the original theme so the run leaves the backend tidy.
        s.invoke_action(format!("appearance:palette:{active}").into());
        for _ in 0..67 {
            let now_active = probe
                .get("/api/dashboard/themes")
                .ok()
                .and_then(|v| v.get("active").and_then(|a| a.as_str()).map(str::to_owned));
            if now_active.as_deref() == Some(active.as_str()) {
                break;
            }
            sleep(300).await;
        }
        record(
            log,
            "widgets: picking a palette switches the backend theme",
            switched,
            format!("picked {pick:?}; GET /api/dashboard/themes active was {pick:?} (restored {active:?})"),
        );
    } else {
        record(
            log,
            "widgets: picking a palette switches the backend theme",
            false,
            "no alternative theme offered",
        );
    }

    // ── appearance/typography: installed-font pickers ───────────────────────────
    s.invoke_goto("appearance".into(), "typography".into());
    let fonts = wait_until(20, || {
        a.get_chat_fonts().row_count() > 0 && a.get_terminal_fonts().row_count() > 0
    })
    .await;
    let family = a
        .get_chat_fonts()
        .row_data(0)
        .map(|f| f.family.to_string())
        .unwrap_or_default();
    record(
        log,
        "widgets: Appearance › Typography lists installed fonts",
        fonts && !family.is_empty(),
        format!(
            "chat fonts={}, terminal fonts={}, first={family:?}, chat current={:?}, fonts-error={:?}",
            a.get_chat_fonts().row_count(),
            a.get_terminal_fonts().row_count(),
            a.get_chat_current(),
            a.get_fonts_error()
        ),
    );

    // ── models/main: the default-model picker ───────────────────────────────────
    let m = ui.global::<hermes_ui::ModelsState>();
    s.invoke_goto("model".into(), "main".into());
    let groups = wait_until(20, || m.get_main_groups().row_count() > 0).await;
    let mut pick: Option<(String, String)> = None;
    for g in m.get_main_groups().iter() {
        for model in g.models.iter() {
            let pair = (g.slug.to_string(), model.to_string());
            if m.get_main_model() != pair.1 || m.get_main_provider() != pair.0 {
                pick = Some(pair);
                break;
            }
        }
        if pick.is_some() {
            break;
        }
    }
    record(
        log,
        "widgets: Model › Main lists providers and models",
        groups && !m.get_main_count().is_empty() && pick.is_some(),
        format!(
            "groups={}, count={:?}, current={} · {}, wait={:?}",
            m.get_main_groups().row_count(),
            m.get_main_count(),
            m.get_main_provider(),
            m.get_main_model(),
            pick
        ),
    );
    if let Some((provider, model)) = pick {
        s.invoke_action(format!("models:pick:{provider}:{model}").into());
        // The backend canonicalises some ids (e.g. `claude-fable-5.1` -> `claude-fable-5-1`),
        // so panel and pick are compared with dots and dashes folded together.
        let fold = |v: &str| v.replace('.', "-");
        let want = fold(&model);
        let ui_ok = wait_until(25, || fold(m.get_main_model().as_ref()) == want).await;
        let mut backend_ok = false;
        for _ in 0..67 {
            let current_provider = probe.get("/api/model/options").ok().and_then(|v| {
                v.get("providers").and_then(|p| p.as_array()).and_then(|list| {
                    list.iter()
                        .find(|p| p.get("is_current").and_then(|c| c.as_bool()).unwrap_or(false))
                        .and_then(|p| p.get("slug").and_then(|s| s.as_str()).map(str::to_owned))
                })
            });
            if current_provider.as_deref() == Some(provider.as_str()) {
                backend_ok = true;
                break;
            }
            sleep(300).await;
        }
        record(
            log,
            "widgets: Model › Main sets the default model (backed by /api/model/set)",
            ui_ok && backend_ok,
            format!(
                "picked {provider} · {model}; UI shows it={ui_ok}; /api/model/options marks that provider current={backend_ok}"
            ),
        );
    }

    // ── model/auxiliary + moa + fallbacks ──────────────────────────────────────
    s.invoke_goto("model".into(), "auxiliary".into());
    let aux = wait_until(20, || m.get_aux_rows().row_count() > 0).await;
    let aux_backend = probe
        .get("/api/model/auxiliary")
        .ok()
        .and_then(|v| v.get("tasks").and_then(|t| t.as_array().map(|a| a.len())))
        .unwrap_or(0);
    record(
        log,
        "widgets: Model › Auxiliary lists every backend task",
        aux && aux_backend > 0 && m.get_aux_rows().row_count() == aux_backend,
        format!("rows={}, backend tasks={aux_backend}", m.get_aux_rows().row_count()),
    );

    s.invoke_goto("model".into(), "moa".into());
    let moa = wait_until(20, || m.get_moa_loaded()).await;
    let moa_backend = probe
        .get("/api/model/moa")
        .ok()
        .and_then(|v| v.get("presets").and_then(|p| p.as_object().map(|o| o.len())))
        .unwrap_or(0);
    record(
        log,
        "widgets: Model › MoA loads presets, reference models and the aggregator",
        moa && moa_backend > 0 && m.get_moa_presets().row_count() == moa_backend,
        format!(
            "loaded={moa}, presets={} (backend {moa_backend}), refs={}, aggregator={} · {}, enabled={}, error={:?}",
            m.get_moa_presets().row_count(),
            m.get_moa_refs().row_count(),
            m.get_moa_aggregator_provider(),
            m.get_moa_aggregator_model(),
            m.get_moa_enabled(),
            m.get_error()
        ),
    );

    s.invoke_goto("model".into(), "fallbacks".into());
    sleep(600).await;
    let fb_rows = m.get_fb_rows().row_count();
    let fb_backend = probe
        .get("/api/config")
        .ok()
        .and_then(|v| v.get("fallback_providers").and_then(|f| f.as_array().map(|a| a.len())))
        .unwrap_or(0);
    record(
        log,
        "widgets: Model › Fallbacks shows the config's fallback list",
        fb_rows == fb_backend,
        format!(
            "editor rows={fb_rows}, config fallback_providers={fb_backend}, error={:?}",
            m.get_error()
        ),
    );

    // ── providers/accounts ──────────────────────────────────────────────────────
    let p = ui.global::<hermes_ui::ProvidersState>();
    s.invoke_goto("providers".into(), "accounts".into());
    let accounts = wait_until(20, || p.get_oauth_rows().row_count() > 0).await;
    let accounts_backend = probe
        .get("/api/providers/oauth")
        .ok()
        .and_then(|v| v.get("providers").and_then(|x| x.as_array().map(|a| a.len())))
        .unwrap_or(0);
    record(
        log,
        "widgets: Providers › Accounts lists every OAuth provider with its sign-in state",
        accounts && p.get_oauth_rows().row_count() == accounts_backend && accounts_backend > 0,
        format!(
            "rows={} (backend {accounts_backend}), status={:?}, status-error={}",
            p.get_oauth_rows().row_count(),
            p.get_status(),
            p.get_status_error()
        ),
    );

    // ── providers/keys ──────────────────────────────────────────────────────────
    s.invoke_goto("providers".into(), "keys".into());
    let keys = wait_until(20, || p.get_key_groups().row_count() > 0).await;
    let env_backend = probe
        .get("/api/env")
        .ok()
        .and_then(|v| v.as_object().map(|o| o.len()))
        .unwrap_or(0);
    record(
        log,
        "widgets: Providers › Keys lists the provider credential groups",
        keys && p.get_key_groups().row_count() > 0 && env_backend > 0,
        format!(
            "groups={}, backend env entries={env_backend}, status={:?}",
            p.get_key_groups().row_count(),
            p.get_status()
        ),
    );

    // ── keys/tools: a real toggle write with read-back ──────────────────────────
    s.invoke_goto("keys".into(), "tools".into());
    let tools = wait_until(20, || p.get_toolset_rows().row_count() > 0).await;
    let first = p.get_toolset_rows().row_data(0);
    let toggle = first.as_ref().map(|t| (t.name.to_string(), t.enabled));
    let mut toggled_ok = false;
    if let Some((name, was)) = toggle.clone()
        && !name.is_empty()
    {
        s.invoke_action(format!("providers:tools-toggle:{name}:{}", !was).into());
        for _ in 0..67 {
            let flipped = probe.get("/api/tools/toolsets").ok().and_then(|v| {
                v.as_array().map(|list| {
                    list.iter().any(|t| {
                        t.get("name").and_then(|n| n.as_str()) == Some(name.as_str())
                            && t.get("enabled").and_then(|e| e.as_bool()) == Some(!was)
                    })
                })
            });
            if flipped == Some(true) {
                toggled_ok = true;
                break;
            }
            sleep(300).await;
        }
        // Put it back.
        s.invoke_action(format!("providers:tools-toggle:{name}:{was}").into());
        for _ in 0..67 {
            let restored = probe.get("/api/tools/toolsets").ok().and_then(|v| {
                v.as_array().map(|list| {
                    list.iter().any(|t| {
                        t.get("name").and_then(|n| n.as_str()) == Some(name.as_str())
                            && t.get("enabled").and_then(|e| e.as_bool()) == Some(was)
                    })
                })
            });
            if restored == Some(true) {
                break;
            }
            sleep(300).await;
        }
    }
    record(
        log,
        "widgets: Keys › Tools toggles a toolset (backed by /api/tools/toolsets)",
        tools && toggled_ok,
        format!(
            "rows={}, toggle={toggle:?}, read-back flipped={toggled_ok}",
            p.get_toolset_rows().row_count()
        ),
    );

    // ── vault/credentials ───────────────────────────────────────────────────────
    let pool_backend = probe
        .get("/api/credentials/pool")
        .ok()
        .and_then(|v| v.get("providers").and_then(|x| x.as_array().map(|a| a.len())))
        .unwrap_or(0);
    s.invoke_goto("vault".into(), "credentials".into());
    let pool = wait_until(20, || {
        p.get_pool_groups().row_count() == pool_backend && pool_backend > 0
    })
    .await;
    record(
        log,
        "widgets: Vault › Saved credentials shows the credential pool",
        pool && p.get_pool_groups().row_count() == pool_backend,
        format!(
            "groups={} (backend providers {pool_backend}), status={:?}",
            p.get_pool_groups().row_count(),
            p.get_status()
        ),
    );

    // ── system: about/updates ───────────────────────────────────────────────────
    let y = ui.global::<hermes_ui::SystemState>();
    s.invoke_goto("about".into(), "updates".into());
    let about = wait_until(20, || !y.get_backend_version().is_empty()).await;
    let status_version = probe
        .get("/api/status")
        .ok()
        .and_then(|v| v.get("version").and_then(|x| x.as_str()).map(str::to_owned))
        .unwrap_or_default();
    record(
        log,
        "widgets: About › Version & updates reads /api/status and the update check",
        about && y.get_backend_version() == status_version && !y.get_install_method().is_empty(),
        format!(
            "client={:?}, backend={:?} (api/status {status_version:?}), install={:?}, update-state={:?}, can-apply={}",
            y.get_client_version(),
            y.get_backend_version(),
            y.get_install_method(),
            y.get_update_state(),
            y.get_can_apply()
        ),
    );

    // ── system: sessions/archived ───────────────────────────────────────────────
    s.invoke_goto("sessions".into(), "archived".into());
    let sessions = wait_until(20, || !y.get_sessions_summary().is_empty()).await;
    let empty_backend = probe
        .get("/api/sessions/empty/count")
        .ok()
        .and_then(|v| v.get("count").and_then(|c| c.as_i64()))
        .unwrap_or(-1) as i32;
    record(
        log,
        "widgets: Sessions › Archive reads the archive stats and the cleanup counts",
        sessions && y.get_empty_count() == empty_backend,
        format!(
            "summary={:?}, empty-count={} (backend {empty_backend}), rows={}, error={:?}",
            y.get_sessions_summary(),
            y.get_empty_count(),
            y.get_archived().row_count(),
            y.get_error()
        ),
    );

    // ── sessions: archive by REST (fenced, the backend's own contract) and restore
    //    through the panel — the write path the row actually offers ────────────────
    // A previous run may have left the seeded chat archived (that is the state this check
    // creates); unarchive it first so the round trip starts from a clean slate.
    let first_id = |v: &serde_json::Value| -> Option<String> {
        v.get("sessions")
            .and_then(|s| s.as_array())
            .and_then(|a| a.first())
            .and_then(|s| s.get("id").and_then(|i| i.as_str()).map(str::to_owned))
    };
    let mut sid = probe.get("/api/sessions?limit=3").ok().and_then(|v| first_id(&v));
    if sid.is_none() {
        sid = probe
            .get("/api/sessions?archived=only&order=recent&limit=3")
            .ok()
            .and_then(|v| first_id(&v));
        if let Some(id) = &sid {
            let rev = probe
                .get(&format!("/api/sessions/{id}/mutation-snapshot"))
                .ok()
                .and_then(|v| v.get("runtime_revision").and_then(|r| r.as_i64()))
                .unwrap_or(0);
            let _ = probe.patch(
                &format!("/api/sessions/{id}"),
                &serde_json::json!({
                    "archived": false,
                    "request_id": format!("widgets-live-reset-{}", std::process::id()),
                    "expected_revision": rev,
                }),
            );
        }
    }
    let mut round_trip = false;
    let mut detail = String::from("no session to archive on this backend");
    if let Some(sid) = sid {
        let rev = probe
            .get(&format!("/api/sessions/{sid}/mutation-snapshot"))
            .ok()
            .and_then(|v| v.get("runtime_revision").and_then(|r| r.as_i64()))
            .unwrap_or(0);
        let archived_backend = probe
            .patch(
                &format!("/api/sessions/{sid}"),
                &serde_json::json!({
                    "archived": true,
                    "request_id": format!("widgets-live-archive-{}", std::process::id()),
                    "expected_revision": rev,
                }),
            )
            .is_ok();
        s.invoke_action("system:sessions-refresh".into());
        let listed = wait_until(20, || {
            y.get_archived()
                .iter()
                .any(|r| r.id == sid && r.title.to_string().contains("WIDGETS"))
        })
        .await;
        s.invoke_action(format!("system:sessions-restore:{sid}").into());
        let restored_ui = wait_until(20, || !y.get_archived().iter().any(|r| r.id == sid)).await;
        let restored_backend = wait_until(20, || {
            probe
                .get("/api/sessions/stats")
                .ok()
                .and_then(|v| v.get("archived").and_then(|a| a.as_i64()))
                == Some(0)
        })
        .await;
        round_trip = archived_backend && listed && restored_ui && restored_backend;
        detail = format!(
            "{sid}: archived via REST={archived_backend}, listed in the panel={listed}, \
             restore via panel={restored_ui}, backend archived=0 again={restored_backend}"
        );
    }
    record(
        log,
        "widgets: Sessions › a chat archives and restores through the panel",
        round_trip,
        detail,
    );

    // ── system: plugins ─────────────────────────────────────────────────────────
    s.invoke_goto("plugins".into(), "installed".into());
    let plugins = wait_until(20, || y.get_plugins().row_count() > 0).await;
    let plugins_backend = probe
        .get("/api/dashboard/plugins")
        .ok()
        .and_then(|v| v.as_array().map(|a| a.len()))
        .unwrap_or(0);
    record(
        log,
        "widgets: Plugins › Desktop plugins lists the installed plugins",
        plugins && y.get_plugins().row_count() == plugins_backend && plugins_backend > 0,
        format!(
            "rows={} (backend {plugins_backend}), note={:?}",
            y.get_plugins().row_count(),
            y.get_plugins_note()
        ),
    );

    // ── system: billing + keybinds + gateway ────────────────────────────────────
    s.invoke_goto("billing".into(), "overview".into());
    let billing = wait_until(20, || !y.get_usage_tokens().is_empty()).await;
    record(
        log,
        "widgets: Billing › Overview shows the portal state and usage totals",
        billing && y.get_usage_days() > 0,
        format!(
            "tokens={:?}, cost={:?}, sessions={:?}, window={}d, models={}, portal={:?}",
            y.get_usage_tokens(),
            y.get_usage_cost(),
            y.get_usage_sessions(),
            y.get_usage_days(),
            y.get_usage_models().row_count(),
            y.get_portal_url()
        ),
    );

    s.invoke_goto("keybinds".into(), "shortcuts".into());
    let keybinds = wait_until(20, || y.get_keybinds().row_count() > 0).await;
    record(
        log,
        "widgets: Keyboard Shortcuts › Key bindings lists the app's real bindings",
        keybinds,
        format!(
            "rows={}, first={:?}",
            y.get_keybinds().row_count(),
            y.get_keybinds()
                .row_data(0)
                .map(|k| format!("{} · {}", k.keys, k.label))
        ),
    );

    s.invoke_goto("gateway".into(), "connection".into());
    let gateway = wait_until(20, || !y.get_gw_endpoint().is_empty()).await;
    let port = std::env::var("HERMES_DESKTOP_PORT").unwrap_or_default();
    record(
        log,
        "widgets: Gateways › This window shows the live backend identity",
        gateway && y.get_gw_endpoint().to_string().contains(&port) && !y.get_gw_version().is_empty(),
        format!(
            "endpoint={:?} (port {port}), version={:?}, protocol={:?}, pid={:?}, state={:?}",
            y.get_gw_endpoint(),
            y.get_gw_version(),
            y.get_gw_protocol(),
            y.get_gw_pid(),
            y.get_gw_state()
        ),
    );

    ui.set_settings_open(false);
    sleep(200).await;
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
        // Queue recovery state: without this a run without HERMES_HOME would write the
        // user's real ~/.local/state/hermes-desktop-rs/durable-state.json.
        std::env::set_var("HERMES_DESKTOP_STATE", scratch.join("durable-state.json"));
    }
    if std::env::var_os("HERMES_LIVE_DISPLAY_PREFS").is_some() {
        chat_display::seed_preferences(&scratch.join("prefs.json"));
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
