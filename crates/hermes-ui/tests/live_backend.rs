//! Live end-to-end test: the real UI (Slint testing backend, no window, no display) driven
//! against a real Hermes backend. Covers the PARITY items that need a live run:
//! composer keys (Enter / Shift+Enter / IME composition), Stop mid-stream (button and Esc),
//! model switching mid-session, the approval card (Deny / Allow once / Allow for session /
//! Always allow, by click and by keyboard), a long (600-message) conversation, and the
//! settings panel (registry nav, "Applies to" profile scope, saving, persistence, search).
//!
//! `HERMES_LIVE_ONLY=keys,stop,model,approval,long,sessions,settings` runs a subset. The
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
    if want("rich") {
        rich_section(&ui, &log).await;
    }
    if want("long") {
        long_section(&ui, &log).await;
    }
    if want("sessions") {
        sessions_section(&ui, &log).await;
    }
    if want("settings") {
        settings_section(&ui, &log).await;
    }
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

#[test]
#[ignore = "needs a running Hermes backend (HERMES_DESKTOP_PORT); see module docs"]
fn live_backend() {
    assert!(
        std::env::var_os("HERMES_DESKTOP_PORT").is_some(),
        "set HERMES_DESKTOP_PORT to a private test backend"
    );
    i_slint_backend_testing::init_integration_test_with_system_time();
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
