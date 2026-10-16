//! Live end-to-end test: the real UI (Slint testing backend, no window, no display) driven
//! against a real Hermes backend. Covers the PARITY items that need a live run:
//! composer keys (Enter / Shift+Enter / IME composition), Stop mid-stream, model switching
//! mid-session, and the approval card (deny, then allow once).
//!
//! Ignored by default: it needs a running backend and spends a few cheap model calls.
//! Run against a *private* backend with approvals enabled (never your everyday one):
//!
//! ```sh
//! scripts/live-test-backend.sh   # prints HERMES_DESKTOP_PORT=…
//! SLINT_EMIT_DEBUG_INFO=1 HERMES_DESKTOP_PORT=<port> cargo test -p hermes-ui --test live_backend -- --ignored --nocapture
//! ```
//!
//! The approval part runs `rm -r` on throwaway directories under
//! `$HERMES_LIVE_SCRATCH` (default `$TMPDIR/hermes-live-test`).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use hermes_ui::{AppState, AppWindow};
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

async fn script(ui: slint::Weak<AppWindow>, log: Log) {
    let ui = ui.upgrade().expect("window");
    let st = ui.global::<AppState>();

    // --- connect
    let ok = wait_until(30, || st.get_connection() == "open").await;
    record(&log, "connect", ok, format!("connection = {}", st.get_connection()));
    if !ok {
        return;
    }

    // --- composer keys
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

    // --- Enter sends; Stop mid-stream
    let prompt = "Write the numbers from one to three hundred as English words, one per line. No tools, no preamble.";
    by_label(&ui, "Message").unwrap().set_accessible_value(prompt);
    press(&ui, Key::Return);
    let sent = wait_until(10, || rows(&ui).first().is_some_and(|r| r.kind == 0)).await;
    record(
        &log,
        "Enter sends",
        sent && composer_text(&ui).is_empty(),
        format!(
            "first row = {:?}, composer = {:?}",
            rows(&ui).first().map(|r| r.text.to_string()),
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

    // --- model switch mid-session
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
                "model switch mid-session",
                switched && ok && reply.to_lowercase().contains("pong") && st.get_model_label() == short.as_str(),
                format!(
                    "{} → {} (label {label_before} → {}); reply after switch = {:?}",
                    c.model,
                    t.model,
                    st.get_model_label(),
                    reply.trim()
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

    // --- approval card: deny, then allow once
    let scratch = std::env::var("HERMES_LIVE_SCRATCH").unwrap_or_else(|_| {
        format!(
            "{}/hermes-live-test",
            std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into())
        )
    });
    for (choice, label) in [("deny", "Deny"), ("once", "Allow once")] {
        let dir = format!("{scratch}/victim-{choice}");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(format!("{dir}/file.txt"), "throwaway").unwrap();
        let prompt = format!(
            "Use the terminal tool to run exactly this command and nothing else: rm -r {dir} \
             — If the command is denied, do not try any alternative; just tell me it was denied."
        );
        let before = rows(&ui).len();
        by_label(&ui, "Message").unwrap().set_accessible_value(prompt.as_str());
        press(&ui, Key::Return);
        let shown = wait_until(120, || approval_visible(&ui)).await;
        let card = st.get_approval();
        if shown && let Some(b) = by_label(&ui, label) {
            b.single_click(PointerEventButton::Left).await;
        }
        let hidden = wait_until(5, || !approval_visible(&ui)).await;
        // The turn is done when it stops running AND a new assistant reply exists after the prompt.
        let reply_after = |ui: &AppWindow| {
            rows(ui)
                .iter()
                .skip(before)
                .rev()
                .find(|r| r.kind == 1 && !r.text.trim().is_empty())
                .map(|r| r.text.to_string())
        };
        let done = wait_until(120, || !running(&ui) && reply_after(&ui).is_some()).await;
        let exists = std::path::Path::new(&dir).exists();
        let reply = reply_after(&ui).unwrap_or_default();
        let expect_exists = choice == "deny";
        record(
            &log,
            &format!("approval card: {label}"),
            shown && hidden && done && exists == expect_exists && idle_clean(&ui).is_ok(),
            format!(
                "card shown={shown} command={:?}; hidden after click={hidden}; turn done={done}; \
                 dir exists afterwards={exists}; agent said: {:?}",
                card.command.to_string(),
                reply.trim().chars().take(240).collect::<String>()
            ),
        );
    }
}

#[test]
#[ignore = "needs a running Hermes backend (HERMES_DESKTOP_PORT); see module docs"]
fn live_backend() {
    assert!(
        std::env::var_os("HERMES_DESKTOP_PORT").is_some(),
        "set HERMES_DESKTOP_PORT to a private test backend"
    );
    i_slint_backend_testing::init_integration_test_with_system_time();
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
