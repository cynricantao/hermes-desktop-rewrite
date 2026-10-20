//! When the backend gives no stable data-root identity, queue recovery is off but the
//! in-memory queue must keep working as it did before persistence existed. Runs against
//! the local gateway fixture only; no real backend, profile or state file is touched.
use hermes_ui::{AppState, AppWindow};
use i_slint_backend_testing::ElementHandle;
use serde_json::Value;
use slint::{
    ComponentHandle,
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
async fn submissions(port: u16, text: &'static str) -> usize {
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let calls: Value = rt
            .block_on(hermes_core::http::Rest::new("127.0.0.1", port, "fixture").get("/api/test/state"))
            .unwrap();
        let n = calls
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["method"] == "prompt.submit" && c["params"]["text"] == text)
            .count();
        let _ = tx.send(n);
    });
    rx.await.unwrap()
}

async fn exercise(ui: &AppWindow, port: u16, path: std::path::PathBuf, checks: Rc<RefCell<Vec<bool>>>) {
    let check = |name: &str, ok: bool| {
        println!("[{}] {name}", if ok { "PASS" } else { "FAIL" });
        checks.borrow_mut().push(ok);
    };
    let state = ui.global::<AppState>();
    check("fixture connected", wait(|| state.get_connection() == "open").await);
    check(
        "missing identity says recovery is off but queues still work",
        wait(|| state.get_banner().contains("Queue recovery is off")).await,
    );
    let input = ElementHandle::find_by_accessible_label(ui, "Message").next();
    let typed = input.is_some_and(|input| {
        input.set_accessible_value("/queue memory-only-prompt");
        ui.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::Return.into(),
        });
        ui.window().dispatch_event(WindowEvent::KeyReleased {
            text: Key::Return.into(),
        });
        true
    });
    let mut submitted = 0;
    if typed {
        for _ in 0..100 {
            submitted = submissions(port, "memory-only-prompt").await;
            if submitted > 0 {
                break;
            }
            pause(40).await;
        }
    }
    check("an idle queued prompt is accepted and submitted once", submitted == 1);
    check(
        "nothing is written when recovery is off",
        !path.exists() && !state.get_banner().contains("not accepted"),
    );
    slint::quit_event_loop().unwrap();
}

#[test]
fn queue_works_in_memory_without_backend_identity() {
    let mut fixture = Fixture(
        Command::new("python3")
            .arg(format!(
                "{}/tests/fixtures/command_gateway.py",
                env!("CARGO_MANIFEST_DIR")
            ))
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .env("FIXTURE_NO_DATA_ROOT", "1")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut line = String::new();
    BufReader::new(fixture.0.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let port: u16 = line.trim().parse().unwrap();
    let scratch = std::env::temp_dir().join(format!("hermes-durable-unavailable-{}", std::process::id()));
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
    let state_path = path.clone();
    slint::spawn_local(async move {
        exercise(&weak.upgrade().unwrap(), port, state_path, checks).await;
    })
    .unwrap();
    slint::run_event_loop().unwrap();
    rt.shutdown_timeout(Duration::from_secs(2));
    assert!(
        results.borrow().iter().all(|value| *value),
        "in-memory queue requirement failed"
    );
    println!("{} recovery-off checks passed", results.borrow().len());
    std::fs::remove_dir_all(scratch).unwrap();
}
