//! Live end-to-end test: the real UI (Slint testing backend, no window, no display) driven
//! against a real Hermes backend. Covers the PARITY items that need a live run:
//! composer keys (Enter / Shift+Enter / IME composition), Stop mid-stream (button and Esc),
//! model switching mid-session, the approval card (Deny / Allow once / Allow for session /
//! Always allow, by click and by keyboard), and a long (600-message) conversation.
//!
//! `HERMES_LIVE_ONLY=keys,stop,model,approval,long,sessions` runs a subset. The `long` section
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
//! The approval part runs `rm -r` on throwaway directories under
//! `$HERMES_LIVE_SCRATCH` (default `$TMPDIR/hermes-live-test`). "Always allow" writes
//! `command_allowlist` into the private backend's config.yaml
//! (`$HERMES_LIVE_HOME`, default `$TMPDIR/hermes-live-home`), which
//! `scripts/live-test-backend.sh` regenerates on every start.

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
    let live_home = std::env::var("HERMES_LIVE_HOME").unwrap_or_else(|_| format!("{}/hermes-live-home", tmpdir()));
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
