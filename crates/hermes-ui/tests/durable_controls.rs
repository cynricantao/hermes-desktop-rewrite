//! Public controller recovery and save-failure checks against the local gateway fixture.
//! No real backend, provider, profile, user prefs or session store is used.
use hermes_core::{
    commands::PendingPrompt,
    durable::{self, DurableState},
};
use hermes_ui::{AppState, AppWindow};
use i_slint_backend_testing::ElementHandle;
use serde_json::Value;
use slint::{
    ComponentHandle, Model,
    platform::{Key, WindowEvent},
};
use std::{
    cell::RefCell,
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    rc::Rc,
    time::{Duration, Instant},
};

struct Fixture(Child);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
async fn pause(ms: u64) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    slint::Timer::single_shot(Duration::from_millis(ms), move || {
        let _ = tx.send(());
    });
    let _ = rx.await;
}
async fn wait(mut check: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(8);
    while Instant::now() < end {
        if check() {
            return true;
        }
        pause(40).await;
    }
    check()
}
fn output(ui: &AppWindow) -> String {
    ui.global::<AppState>()
        .get_rows()
        .iter()
        .map(|r| r.text.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}
fn send(ui: &AppWindow, text: &str) -> bool {
    let Some(input) = ElementHandle::find_by_accessible_label(ui, "Message").next() else {
        return false;
    };
    input.set_accessible_value(text);
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Return.into(),
    });
    ui.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Return.into(),
    });
    true
}
async fn calls(port: u16) -> Value {
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt
            .block_on(hermes_core::http::Rest::new("127.0.0.1", port, "fixture").get("/api/test/state"))
            .unwrap();
        let _ = tx.send(result);
    });
    rx.await.unwrap()
}
fn submissions(calls: &Value, text: &str) -> usize {
    calls
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["method"] == "prompt.submit" && c["params"]["text"] == text)
        .count()
}

async fn exercise(
    ui: &AppWindow,
    port: u16,
    path: std::path::PathBuf,
    backend: String,
    checks: Rc<RefCell<Vec<bool>>>,
) {
    let check = |name: &str, ok: bool| {
        println!("[{}] {name}", if ok { "PASS" } else { "FAIL" });
        checks.borrow_mut().push(ok);
    };
    let state = ui.global::<AppState>();
    check("fixture connected", wait(|| state.get_connection() == "open").await);
    state.invoke_open_session("stored-recovery".into());
    check(
        "restart restores queue paused and renders saved background result",
        wait(|| state.get_queue_count() == 1 && output(ui).contains("recovered background answer")).await,
    );
    check(
        "restored queue does not automatically submit",
        submissions(&calls(port).await, "queued-recovered") == 0,
    );
    let saved = DurableState::open(&path, &backend).unwrap();
    check(
        "showing the background notice does not delete its durable copy",
        saved.results("stored-recovery").len() == 1,
    );
    state.invoke_new_chat();
    wait(|| state.get_active_session().is_empty()).await;
    state.invoke_open_session("stored-recovery".into());
    check(
        "reopening hydrates one copy of the saved background notice",
        wait(|| output(ui).matches("recovered background answer").count() == 1).await,
    );
    let typed = send(ui, "/queue resume");
    check(
        "explicit resume dispatches and removes one queued prompt",
        typed && wait(|| state.get_queue_count() == 0 && state.get_running()).await,
    );
    check(
        "resume submits exactly once",
        submissions(&calls(port).await, "queued-recovered") == 1,
    );
    let saved = DurableState::open(&path, &backend).unwrap();
    check(
        "queue pop and uncertain in-flight record were saved together",
        saved.entries("stored-recovery").is_empty() && saved.pending("stored-recovery").is_some(),
    );
    state.invoke_open_session("stored-blocked".into());
    check(
        "second stored chat restores its own queue",
        wait(|| state.get_active_session() == "stored-blocked" && state.get_queue_count() == 1).await,
    );
    // A separate opener successfully changes the file. The running controller now
    // holds a stale snapshot and must neither overwrite it nor dispatch its FIFO.
    let mut external = DurableState::open(&path, &backend).unwrap();
    external
        .set_queue(
            "another-instance",
            vec![PendingPrompt {
                text: "other".into(),
                display: "other".into(),
            }],
            true,
        )
        .unwrap();
    external.save().unwrap();
    let before = std::fs::read(&path).unwrap();
    let typed = send(ui, "/queue resume");
    check(
        "stale-writer save failure pauses delivery and surfaces a visible error",
        typed && wait(|| state.get_banner().contains("another app instance")).await && state.get_queue_count() == 1,
    );
    check(
        "failed save sends nothing and keeps the other instance's exact file",
        submissions(&calls(port).await, "must-not-submit") == 0 && std::fs::read(&path).unwrap() == before,
    );
    slint::quit_event_loop().unwrap();
}

#[test]
fn durable_controller_recovery_and_conflict() {
    let mut fixture = Fixture(
        Command::new("python3")
            .arg(format!(
                "{}/tests/fixtures/command_gateway.py",
                env!("CARGO_MANIFEST_DIR")
            ))
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut line = String::new();
    BufReader::new(fixture.0.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let port: u16 = line.trim().parse().unwrap();
    let scratch = std::env::temp_dir().join(format!("hermes-durable-controller-{}", std::process::id()));
    std::fs::create_dir(&scratch).unwrap();
    let path = scratch.join("state.json");
    unsafe {
        std::env::set_var("HERMES_DESKTOP_PORT", port.to_string());
        std::env::set_var("HERMES_HOME", &scratch);
        std::env::set_var("HERMES_DESKTOP_STATE", &path);
        std::env::set_var("HERMES_DESKTOP_PREFS", scratch.join("prefs.json"));
        std::env::set_var("HERMES_BIN", "/nonexistent-fixture-must-not-spawn-hermes");
        for name in [
            "DISPLAY",
            "WAYLAND_DISPLAY",
            "HERMES_DESKTOP_AUTOSEND",
            "HERMES_DESKTOP_OPEN_SESSION",
        ] {
            std::env::remove_var(name);
        }
    }
    let backend = durable::backend_id(std::path::Path::new("/fixture/backend"), None);
    let mut store = DurableState::open(&path, &backend).unwrap();
    for (chat, text) in [
        ("stored-recovery", "queued-recovered"),
        ("stored-blocked", "must-not-submit"),
    ] {
        store
            .set_queue(
                chat,
                vec![PendingPrompt {
                    text: text.into(),
                    display: text.into(),
                }],
                false,
            )
            .unwrap();
    }
    store
        .record_result(
            "stored-recovery",
            "fixture-bg-result",
            "fixture-bg",
            "recovered background answer",
            false,
        )
        .unwrap();
    store.save().unwrap();
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: false,
            threading: true,
            renderer_name: None,
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
    let results = Rc::new(RefCell::new(Vec::new()));
    let checks = results.clone();
    let weak = ui.as_weak();
    slint::spawn_local(async move {
        exercise(&weak.upgrade().unwrap(), port, path, backend, checks).await;
    })
    .unwrap();
    slint::run_event_loop().unwrap();
    rt.shutdown_timeout(Duration::from_secs(2));
    assert!(
        results.borrow().iter().all(|value| *value),
        "durable controller requirement failed"
    );
    println!("{} durable controller checks passed", results.borrow().len());
    std::fs::remove_dir_all(scratch).unwrap();
}
