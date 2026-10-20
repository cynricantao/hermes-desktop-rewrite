//! Daily-driver controls (agent question cards, …) driven through the real Slint window and
//! controller against the deterministic gateway simulation in `fixtures/command_gateway.py`.
//! No Hermes backend, model, provider, credentials or tools are used here.
use std::cell::RefCell;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::time::{Duration, Instant};

use hermes_ui::{AppState, AppWindow, InputState};
use i_slint_backend_testing::ElementHandle;
use serde_json::{Value, json};
use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{ComponentHandle, Model};

struct Fixture(Child);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
async fn sleep(ms: u64) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    slint::Timer::single_shot(Duration::from_millis(ms), move || {
        let _ = tx.send(());
    });
    let _ = rx.await;
}
async fn wait(mut predicate: impl FnMut() -> bool) -> bool {
    let until = Instant::now() + Duration::from_secs(8);
    while Instant::now() < until {
        if predicate() {
            return true;
        }
        sleep(50).await;
    }
    predicate()
}
fn press(ui: &AppWindow, key: Key) {
    ui.window().dispatch_event(WindowEvent::KeyPressed { text: key.into() });
    ui.window()
        .dispatch_event(WindowEvent::KeyReleased { text: key.into() });
}
fn set_field(ui: &AppWindow, label: &str, text: &str) -> bool {
    match ElementHandle::find_by_accessible_label(ui, label).next() {
        Some(e) => {
            e.set_accessible_value(text);
            true
        }
        None => false,
    }
}
async fn click(ui: &AppWindow, label: &str) -> bool {
    sleep(120).await;
    match ElementHandle::find_by_accessible_label(ui, label).next() {
        Some(e) => {
            e.single_click(PointerEventButton::Left).await;
            true
        }
        None => false,
    }
}
fn rows_text(ui: &AppWindow) -> String {
    ui.global::<AppState>()
        .get_rows()
        .iter()
        .map(|r| r.text.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}
/// Headless software-rendered frame, written only when `HERMES_LIVE_SCREENSHOTS` names a dir.
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
async fn fixture_get(port: u16, path: &str) -> Value {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let path = path.to_owned();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let value = rt
            .block_on(hermes_core::http::Rest::new("127.0.0.1", port, "fixture").get(&path))
            .unwrap();
        let _ = tx.send(value);
    });
    rx.await.unwrap()
}
/// The client's reply to server request `id`, if it sent one.
async fn reply(port: u16, id: &str) -> Option<Value> {
    fixture_get(port, "/api/test/state")
        .await
        .as_array()?
        .iter()
        .find(|c| c["method"] == "<response>" && c["params"]["id"] == id)
        .map(|c| c["params"].clone())
}
async fn wait_reply(port: u16, id: &str) -> Option<Value> {
    let until = Instant::now() + Duration::from_secs(8);
    while Instant::now() < until {
        if let Some(r) = reply(port, id).await {
            return Some(r);
        }
        sleep(80).await;
    }
    None
}
async fn ask(port: u16, kind: &str, sid: &str) -> String {
    fixture_get(port, &format!("/api/test/ask?kind={kind}&sid={sid}")).await["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn input_cards(ui: &AppWindow, port: u16, check: &mut impl FnMut(String, bool)) {
    let s = ui.global::<AppState>();
    let card = ui.global::<InputState>();
    set_field(ui, "Message", "start a turn");
    press(ui, Key::Return);
    check(
        "turn running in a real session".into(),
        wait(|| s.get_running() && !s.get_runtime_session().is_empty()).await,
    );
    let sid = s.get_runtime_session().to_string();

    // Batch clarify: single choice, multi-select, open text.
    let id = ask(port, "clarify", &sid).await;
    check(
        "clarify card shows first of three questions".into(),
        wait(|| card.get_visible() && card.get_step() == "1 of 3").await
            && card.get_question() == "Which scope?"
            && card.get_choices().row_count() == 2
            && card
                .get_choices()
                .row_data(0)
                .is_some_and(|c| c.recommended && c.label == "Small"),
    );
    check(
        "pointer picks a single choice and advances".into(),
        click(ui, "Choose Small").await && wait(|| card.get_step() == "2 of 3").await && card.get_multi(),
    );
    let toggled = click(ui, "Choose A").await && click(ui, "Choose C").await;
    sleep(300).await;
    screenshot(ui, "rs-clarify-card");
    check(
        "multi-select toggles stay local until confirmed".into(),
        toggled
            && wait(|| card.get_choices().iter().filter(|c| c.selected).count() == 2).await
            && reply(port, &id).await.is_none(),
    );
    check(
        "confirm advances to the open question".into(),
        click(ui, "Confirm choices").await && wait(|| card.get_step() == "3 of 3").await,
    );
    let typed = set_field(ui, "Answer", "ship it");
    press(ui, Key::Return);
    let r = wait_reply(port, &id).await;
    check(
        format!("batch reply carries every answer: {r:?}"),
        typed
            && r.as_ref().map(|r| &r["result"])
                == Some(&json!({"answers": {"q0": "Small", "q1": "[\"A\",\"C\"]", "q2": "ship it"}}))
            && wait(|| !card.get_visible()).await,
    );

    // Review regressions: a fast double click answers one question only, and text typed for
    // one question does not carry over to the next.
    let id = ask(port, "clarify", &sid).await;
    wait(|| card.get_visible() && card.get_step() == "1 of 3").await;
    sleep(150).await;
    let double = match ElementHandle::find_by_accessible_label(ui, "Choose Large").next() {
        Some(e) => {
            e.single_click(PointerEventButton::Left).await;
            e.single_click(PointerEventButton::Left).await;
            true
        }
        None => false,
    };
    sleep(400).await;
    check(
        "double click answers only the question on screen".into(),
        double && card.get_step() == "2 of 3" && card.get_choices().iter().all(|c| !c.selected),
    );
    let typed = set_field(ui, "Answer", "leftover text");
    let picked = click(ui, "Choose B").await && click(ui, "Confirm choices").await;
    check(
        "typed text is cleared when the question changes".into(),
        typed && picked && wait(|| card.get_step() == "3 of 3").await && card.get_value().is_empty(),
    );
    press(ui, Key::Escape);
    let r = wait_reply(port, &id).await;
    check(
        format!("skipping after partial answers replies {{}}: {r:?}"),
        r.as_ref().map(|r| &r["result"]) == Some(&json!({})),
    );

    // Skip one question of a batch, then Esc declines a whole clarify.
    let id = ask(port, "clarify", &sid).await;
    wait(|| card.get_visible()).await;
    let skipped = click(ui, "Skip question").await && wait(|| card.get_step() == "2 of 3").await;
    press(ui, Key::Escape);
    let r = wait_reply(port, &id).await;
    check(
        format!("Esc skips the rest without stopping the turn: {r:?}"),
        skipped && r.as_ref().map(|r| &r["result"]) == Some(&json!({})) && s.get_running(),
    );

    // Masked prompts: the value goes back once and is never shown.
    let id = ask(port, "sudo", &sid).await;
    check(
        "sudo card is masked and names the command".into(),
        wait(|| card.get_visible() && card.get_masked()).await && card.get_detail().contains("pacman"),
    );
    sleep(300).await;
    screenshot(ui, "rs-sudo-card");
    let typed = set_field(ui, "Answer", "fixture-pass-123");
    press(ui, Key::Return);
    let r = wait_reply(port, &id).await;
    check(
        "sudo password returns as value and leaves no trace".into(),
        typed
            && r.as_ref().map(|r| &r["result"]) == Some(&json!({"value": "fixture-pass-123"}))
            && wait(|| !card.get_visible()).await
            && card.get_value().is_empty()
            && !rows_text(ui).contains("fixture-pass-123")
            && !s.get_draft().contains("fixture-pass-123"),
    );
    let id = ask(port, "vault.code", &sid).await;
    let shown = wait(|| card.get_visible() && card.get_masked()).await && card.get_detail().contains("example.test");
    screenshot(ui, "rs-vault-code-card");
    let typed = set_field(ui, "Answer", "fixture-verification-code");
    press(ui, Key::Return);
    let r = wait_reply(port, &id).await;
    check(
        "vault code uses the masked card, replies once, and clears private text".into(),
        shown
            && typed
            && r.as_ref().map(|r| &r["result"]) == Some(&json!({"value": "fixture-verification-code"}))
            && wait(|| !card.get_visible()).await
            && card.get_value().is_empty()
            && !rows_text(ui).contains("fixture-verification-code")
            && !s.get_draft().contains("fixture-verification-code"),
    );
    let id = ask(port, "vault.code", &sid).await;
    wait(|| card.get_visible() && card.get_masked()).await;
    press(ui, Key::Escape);
    check(
        "Esc declines a vault code without stopping the turn".into(),
        wait_reply(port, &id).await.as_ref().map(|r| &r["result"]) == Some(&json!({"value": ""})) && s.get_running(),
    );
    let id = ask(port, "secret", &sid).await;
    wait(|| card.get_visible()).await;
    let shown = card.get_title() == "Fixture API key" && card.get_detail() == "Saved as FIXTURE_KEY";
    let clicked = click(ui, "Skip").await;
    let r = wait_reply(port, &id).await;
    check(
        "secret card can be skipped with an empty value".into(),
        shown && clicked && r.as_ref().map(|r| &r["result"]) == Some(&json!({"value": ""})),
    );

    // Withdrawn, foreign-chat and unusable requests.
    let id = ask(port, "clarify-one", &sid).await;
    wait(|| card.get_visible()).await;
    fixture_get(port, &format!("/api/test/cancel?id={id}&sid={sid}")).await;
    check(
        "request.cancel tears the card down without a reply".into(),
        wait(|| !card.get_visible()).await && reply(port, &id).await.is_none(),
    );
    let other = ask(port, "clarify-one", "another-runtime").await;
    sleep(400).await;
    check(
        "another chat's question is parked, not shown here".into(),
        !card.get_visible() && s.get_banner().contains("Another chat") && reply(port, &other).await.is_none(),
    );
    fixture_get(port, &format!("/api/test/cancel?id={other}&sid=another-runtime")).await;
    let id = ask(port, "clarify-empty", &sid).await;
    let r = wait_reply(port, &id).await;
    check(
        "clarify without usable questions is answered at once".into(),
        r.as_ref().map(|r| &r["result"]) == Some(&json!({})) && !card.get_visible(),
    );

    // Reopening a chat replays its still-open question at the first unlocked step.
    s.invoke_open_session("stored-replay".into());
    check(
        "resume replays an open clarify at its next question".into(),
        wait(|| card.get_visible() && card.get_request_id() == "srq-replayed").await
            && card.get_question() == "Which parts?"
            && card.get_step() == "2 of 2",
    );
    let picked = click(ui, "Choose B").await && click(ui, "Confirm choices").await;
    let r = wait_reply(port, "srq-replayed").await;
    check(
        format!("replayed batch reply keeps the locked answer: {r:?}"),
        picked && r.as_ref().map(|r| &r["result"]) == Some(&json!({"answers": {"q0": "Small", "q1": "[\"B\"]"}})),
    );
}

async fn script(ui: &AppWindow, port: u16, log: &Rc<RefCell<Vec<bool>>>) {
    let mut check = |name: String, ok: bool| {
        println!("[{}] {name}", if ok { "PASS" } else { "FAIL" });
        log.borrow_mut().push(ok);
    };
    let s = ui.global::<AppState>();
    check(
        "simulated gateway connected".into(),
        wait(|| s.get_connection() == "open").await,
    );
    input_cards(ui, port, &mut check).await;
}

#[test]
fn daily_driver_simulated() {
    let mut fixture = Fixture(
        Command::new("python3")
            .arg(format!(
                "{}/tests/fixtures/command_gateway.py",
                env!("CARGO_MANIFEST_DIR")
            ))
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .stdout(Stdio::piped())
            .spawn()
            .expect("Python fixture"),
    );
    let mut line = String::new();
    BufReader::new(fixture.0.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let port: u16 = line.trim().parse().expect("loopback fixture port");
    let scratch = std::env::temp_dir().join(format!("hermes-daily-driver-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    // This test is its own process. Every endpoint and preference is isolated.
    unsafe {
        std::env::set_var("HERMES_DESKTOP_PORT", port.to_string());
        std::env::set_var("HERMES_HOME", &scratch);
        std::env::set_var("HERMES_BIN", "/nonexistent-fixture-must-not-spawn-hermes");
        std::env::set_var("HERMES_DESKTOP_PREFS", scratch.join("prefs.json"));
        std::env::remove_var("DISPLAY");
        std::env::remove_var("WAYLAND_DISPLAY");
        std::env::remove_var("HERMES_DESKTOP_AUTOSEND");
        std::env::remove_var("HERMES_DESKTOP_OPEN_SESSION");
    }
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: false,
            threading: true,
            renderer_name: std::env::var_os("HERMES_LIVE_SCREENSHOTS").map(|_| "software".into()),
        },
    )))
    .unwrap();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let ui = AppWindow::new().unwrap();
    ui.window().set_size(slint::LogicalSize::new(1440., 900.));
    hermes_ui::start(&ui, &rt);
    ui.show().unwrap();
    let log = Rc::new(RefCell::new(Vec::new()));
    let (weak, checks) = (ui.as_weak(), log.clone());
    slint::spawn_local(async move {
        script(&weak.upgrade().unwrap(), port, &checks).await;
        slint::quit_event_loop().unwrap();
    })
    .unwrap();
    slint::run_event_loop().unwrap();
    rt.shutdown_timeout(Duration::from_secs(2));
    let failed = log.borrow().iter().filter(|v| !**v).count();
    println!("{} simulated checks, {failed} failed", log.borrow().len());
    assert_eq!(failed, 0);
}
