//! Real Slint composer/controller against a deterministic gateway simulation.
//! No Hermes backend, external provider, credentials, or tools are used here.
use std::cell::RefCell;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::time::{Duration, Instant};

use hermes_ui::{AppState, AppWindow};
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
fn input(ui: &AppWindow, text: &str) {
    ElementHandle::find_by_accessible_label(ui, "Message")
        .next()
        .expect("composer")
        .set_accessible_value(text);
}
fn send(ui: &AppWindow, text: &str) {
    input(ui, text);
    press(ui, Key::Return);
}
fn output(ui: &AppWindow) -> String {
    ui.global::<AppState>()
        .get_rows()
        .iter()
        .map(|r| r.text.to_string())
        .collect::<Vec<_>>()
        .join("\n")
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
fn submissions(state: &Value) -> Vec<String> {
    state
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["method"] == "prompt.submit")
        .map(|c| c["params"]["text"].as_str().unwrap().into())
        .collect()
}

async fn script(ui: &AppWindow, port: u16, log: &Rc<RefCell<Vec<bool>>>) {
    macro_rules! check {
        ($name:expr, $ok:expr) => {{
            let ok = $ok;
            println!("[{}] {}", if ok { "PASS" } else { "FAIL" }, $name);
            log.borrow_mut().push(ok);
        }};
    }
    let s = ui.global::<AppState>();
    check!(
        "simulated gateway connected",
        wait(|| s.get_connection() == "open").await
    );
    input(ui, "/he");
    check!(
        "composer gets command suggestions",
        wait(|| s.get_completions().row_count() > 0).await
    );
    sleep(200).await;
    let clicked = if let Some(e) = ElementHandle::find_by_accessible_label(ui, "Complete /help").next() {
        e.single_click(PointerEventButton::Left).await;
        true
    } else {
        false
    };
    check!(
        "pointer selects command without sending",
        clicked
            && wait(|| s.get_draft().as_str() == "/help ").await
            && submissions(&fixture_get(port, "/api/test/state").await).is_empty()
    );
    input(ui, "/fixture-s");
    check!(
        "installed skill appears in suggestions",
        wait(|| s.get_completions().iter().any(|c| c.label.as_str() == "/fixture-skill")).await
    );
    press(ui, Key::Tab);
    check!(
        "Tab fills skill invocation",
        s.get_draft().as_str() == "/fixture-skill "
    );
    input(ui, "plain draft");
    check!(
        "stale completion cannot overwrite plain text",
        wait(|| s.get_completions().row_count() == 0).await && s.get_draft().as_str() == "plain draft"
    );

    send(ui, "hold-first");
    check!("normal submit marks current turn busy", wait(|| s.get_running()).await);
    send(ui, "/q second");
    check!(
        "alias queues without interrupting",
        wait(|| s.get_queue_count() == 1).await && s.get_running()
    );
    input(ui, "unsent draft");
    sleep(200).await;
    let badge = ElementHandle::find_by_accessible_label(ui, "Queued: 1 ").next();
    let inspected = if let Some(b) = badge {
        b.single_click(PointerEventButton::Left).await;
        true
    } else {
        false
    };
    sleep(200).await;
    check!(
        "queue badge preserves unsent draft",
        inspected && s.get_draft().as_str() == "unsent draft"
    );
    send(ui, "busy plain draft");
    check!(
        "plain Enter while busy explains and preserves draft",
        wait(|| s.get_banner().contains("still running") && s.get_draft().as_str() == "busy plain draft").await
    );
    send(ui, "/queue third");
    check!(
        "busy composer accepts another queued command",
        wait(|| s.get_queue_count() == 2).await
    );
    send(ui, "/queue move 2 1");
    sleep(150).await;
    send(ui, "/queue list");
    check!(
        "queue management preserves ordering",
        wait(|| output(ui).contains("1. third\n2. second")).await
    );
    send(ui, "/steer focus-here");
    check!(
        "steer uses live correction without interrupt",
        wait(|| output(ui).contains("Steer queued")).await
    );
    send(ui, "/steer accepted-steer");
    sleep(200).await;
    check!("already-applied steer never queued twice", s.get_queue_count() == 2);
    send(ui, "/steer unknown-steer");
    check!(
        "unknown steer status is not resent",
        wait(|| s.get_banner().contains("unknown steer status")).await && s.get_queue_count() == 2
    );
    let state = fixture_get(port, "/api/test/state").await;
    check!(
        "steer has exact text and runtime",
        state
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["method"] == "session.steer" && c["params"] == json!({"session_id":"s1","text":"focus-here"}))
            && !state
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["method"] == "session.interrupt")
    );
    send(ui, "/background detached");
    check!(
        "background task leaves foreground busy",
        wait(|| output(ui).contains("bg_fixture")).await && s.get_running()
    );
    fixture_get(port, "/api/test/side?sid=s1").await;
    check!(
        "background completion is rendered",
        wait(|| output(ui).contains("Simulated background answer.")).await && s.get_running()
    );
    fixture_get(port, "/api/test/finish?sid=s1").await;
    check!(
        "settling first turn drains one queue entry",
        wait(|| s.get_queue_count() == 1).await
    );
    let state = fixture_get(port, "/api/test/state").await;
    check!(
        "drain sends requested FIFO order",
        submissions(&state) == ["hold-first", "third"]
    );
    fixture_get(port, "/api/test/finish?sid=s1").await;
    check!(
        "next settle drains next prompt",
        wait(|| s.get_queue_count() == 0).await
    );
    check!(
        "FIFO submits once each",
        submissions(&fixture_get(port, "/api/test/state").await) == ["hold-first", "third", "second"]
    );

    input(ui, "control-enter-fixture");
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Control.into(),
    });
    press(ui, Key::Return);
    ui.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Control.into(),
    });
    check!(
        "Ctrl+Enter queues without interrupt",
        wait(|| s.get_queue_count() == 1).await && s.get_running()
    );
    send(ui, "/queue clear");
    wait(|| s.get_queue_count() == 0).await;
    send(ui, "/quick from-quick-alias");
    check!(
        "quick alias expansion reaches local queue",
        wait(|| s.get_queue_count() == 1).await
    );
    send(ui, "/fixture-skill");
    check!(
        "skill dispatch expands once and queues while busy",
        wait(|| s.get_queue_count() == 2).await
    );
    check!(
        "expanded skill instructions not shown",
        !output(ui).contains("PRIVATE_SKILL_BODY")
    );
    send(ui, "/stop");
    sleep(200).await;
    fixture_get(port, "/api/test/finish?sid=s1").await;
    check!(
        "Stop pauses queue rather than restarting",
        wait(|| !s.get_running()).await && s.get_queue_count() == 2
    );
    send(ui, "/queue resume");
    check!(
        "explicit resume continues queued work",
        wait(|| s.get_queue_count() == 1).await
    );
    fixture_get(port, "/api/test/finish?sid=s1").await;
    check!(
        "expanded skill is eventually submitted",
        wait(|| s.get_queue_count() == 0).await
    );
    let state = fixture_get(port, "/api/test/state").await;
    check!(
        "skill body sent privately with short display",
        state.as_array().unwrap().iter().any(|c| c["method"] == "prompt.submit"
            && c["params"]["text"] == "PRIVATE_SKILL_BODY"
            && c["params"]["title_preview"] == "/fixture-skill"
            && c["params"]["display_kind"] == "hidden")
            && !output(ui).contains("PRIVATE_SKILL_BODY")
    );

    send(ui, "/danger-fails");
    check!(
        "failed command surfaces its actual error",
        wait(|| s.get_banner().contains("simulated failure")).await
    );
    let state = fixture_get(port, "/api/test/state").await;
    check!(
        "failure after execution never retries through dispatch",
        !state
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["method"] == "command.dispatch" && c["params"]["name"] == "danger-fails")
    );
    send(ui, "/queue attachment-safe");
    wait(|| s.get_queue_count() == 1).await;
    s.invoke_paste_image();
    check!(
        "simulated image staged after queueing",
        wait(|| s.get_attachments().row_count() == 1).await
    );
    let before_attachment = submissions(&fixture_get(port, "/api/test/state").await).len();
    for command in [
        "/queue important prompt",
        "/q alias prompt",
        "/quick quick-alias prompt",
    ] {
        send(ui, command);
        check!(
            format!("rejected attachment queue retains original {command}"),
            wait(|| s.get_banner().contains("before queuing a prompt") && s.get_draft().as_str() == command).await
                && s.get_queue_count() == 1
                && s.get_attachments().row_count() == 1
        );
    }
    input(ui, "important keyboard queue prompt");
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: Key::Control.into(),
    });
    press(ui, Key::Return);
    ui.window().dispatch_event(WindowEvent::KeyReleased {
        text: Key::Control.into(),
    });
    check!(
        "rejected Ctrl+Enter retains literal draft and never submits",
        wait(|| s.get_draft().as_str() == "important keyboard queue prompt").await
            && s.get_banner().contains("before queuing a prompt")
            && s.get_queue_count() == 1
            && s.get_attachments().row_count() == 1
            && submissions(&fixture_get(port, "/api/test/state").await).len() == before_attachment
    );
    send(ui, "/queue edit 99 replacement");
    check!(
        "invalid queue management retains its invocation",
        wait(|| s.get_draft().as_str() == "/queue edit 99 replacement" && s.get_banner_error()).await
            && s.get_queue_count() == 1
    );
    fixture_get(port, "/api/test/finish?sid=s1").await;
    check!(
        "drain preserves later attachment and pauses",
        wait(|| !s.get_running()).await
            && s.get_queue_count() == 1
            && s.get_attachments().row_count() == 1
            && submissions(&fixture_get(port, "/api/test/state").await).len() == before_attachment
    );
    s.invoke_remove_attachment(0);
    wait(|| s.get_attachments().row_count() == 0).await;
    send(ui, "/queue resume");
    check!(
        "explicit resume after removing image drains safely",
        wait(|| s.get_queue_count() == 0 && s.get_running()).await
    );
    send(ui, "/queue after-error");
    wait(|| s.get_queue_count() == 1).await;
    fixture_get(port, "/api/test/finish?sid=s1&status=error").await;
    check!(
        "completion error pauses pending prompts",
        wait(|| !s.get_running()).await && s.get_queue_count() == 1
    );
    send(ui, "/queue clear");
    wait(|| s.get_queue_count() == 0).await;
    send(ui, "/queue resume");
    sleep(150).await;
    send(ui, "fail-later");
    check!("late-failure request in flight", wait(|| s.get_running()).await);
    // Wait until the fixture received the request before asking it to reject.
    sleep(250).await;
    send(ui, "/queue protected-original-chat");
    wait(|| s.get_queue_count() == 1).await;
    send(ui, "/new");
    check!(
        "new chat clears runtime and queue badge",
        wait(|| s.get_runtime_session().is_empty() && s.get_queue_count() == 0).await
    );
    send(ui, "another-chat");
    check!(
        "new chat has independent runtime",
        wait(|| s.get_runtime_session().as_str() == "s2").await
    );
    fixture_get(port, "/api/test/fail?sid=s1").await;
    check!(
        "inactive submission failure is retained without stopping foreground",
        wait(|| s.get_banner().contains("another chat")).await && s.get_running() && s.get_draft().is_empty()
    );
    fixture_get(port, "/api/test/side?sid=s1").await;
    sleep(200).await;
    check!(
        "background results never enter wrong chat",
        !output(ui).contains("Simulated background answer.")
    );
    s.invoke_open_session("stored-s1".into());
    check!(
        "failed prompt and paused queue survive runtime remint",
        wait(|| s.get_runtime_session().as_str() == "resumed-stored-s1"
            && s.get_queue_count() == 1
            && s.get_draft().as_str() == "fail-later")
        .await
            && !s.get_running()
    );
    check!(
        "reopened history never exposes expanded skill",
        !output(ui).contains("PRIVATE_SKILL_BODY")
    );
    check!(
        "parent receives pending side result on reopen",
        output(ui).contains("Simulated background answer.")
    );
    send(ui, "/queue resume");
    check!(
        "reopened chat drains to its new runtime only",
        wait(|| s.get_queue_count() == 0 && s.get_running()).await
    );
    let state = fixture_get(port, "/api/test/state").await;
    check!(
        "queued original prompt never crosses to another chat",
        state.as_array().unwrap().iter().any(|c| c["method"] == "prompt.submit"
            && c["params"]["text"] == "protected-original-chat"
            && c["params"]["session_id"] == "resumed-stored-s1")
            && !state.as_array().unwrap().iter().any(|c| c["method"] == "prompt.submit"
                && c["params"]["text"] == "protected-original-chat"
                && c["params"]["session_id"] == "s2")
    );
    send(ui, "/quick-loop");
    check!(
        "alias cycle stops with clear error",
        wait(|| s.get_banner().contains("alias loop")).await
    );
    send(ui, "/new");
    check!(
        "queue failure probe starts in an observed clean chat",
        wait(|| s.get_runtime_session().is_empty() && s.get_rows().row_count() == 0).await
    );
    for (draft, keyboard, newer) in [
        ("queue create failure", true, false),
        ("/queue slash create failure", false, false),
        ("queue create failure while typing", true, true),
    ] {
        fixture_get(port, "/api/test/reject-create").await;
        if keyboard {
            input(ui, draft);
            ui.window().dispatch_event(WindowEvent::KeyPressed {
                text: Key::Control.into(),
            });
            press(ui, Key::Return);
            ui.window().dispatch_event(WindowEvent::KeyReleased {
                text: Key::Control.into(),
            });
        } else {
            send(ui, draft);
        }
        let pending = fixture_get(port, "/api/test/wait-create").await;
        if newer {
            input(ui, "newer draft must survive");
        }
        fixture_get(port, "/api/test/fail-create").await;
        let expected = if newer { "newer draft must survive" } else { draft };
        check!(
            format!("session-create queue failure recovers original without replacing newer input: {draft}"),
            pending["pending"] == true
                && wait(|| s.get_banner().contains("Simulated session creation failure")
                    && output(ui).contains(draft)
                    && s.get_draft().as_str() == expected)
                .await
                && s.get_runtime_session().is_empty()
                && s.get_queue_count() == 0
        );
    }
}

#[test]
fn command_controls_simulated() {
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
    let scratch = std::env::temp_dir().join(format!("hermes-command-controls-{}", std::process::id()));
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
