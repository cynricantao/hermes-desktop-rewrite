//! Async controller: owns backend + client + transcript; talks to the UI only through
//! `invoke_from_event_loop` (UI ← core) and an unbounded command channel (UI → core).

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use hermes_core::backend::{self, ManagedBackend};
use hermes_core::protocol::{
    ModelOptionsResult, SessionCreateResult, SessionListResult, SessionListRow, SessionResumeResult,
};
use hermes_core::sessions::{display_title, group_by_age};
use hermes_core::transcript::{Change, Transcript};
use hermes_core::{Client, ClientEvent};
use serde_json::{Value, json};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel, Weak};
use tokio::sync::mpsc;

use crate::rows::to_row;
use crate::{AppState, AppWindow, ApprovalInfo, ChatRow, ModelItem, SessionItem, SettingsState, Theme};

/// Commands from the UI thread to the controller.
#[derive(Debug, Clone)]
pub enum Cmd {
    Send(String),
    Stop,
    NewChat,
    Open(String),
    Delete(String),
    Search(String),
    RefreshModels,
    SelectModel { provider: String, model: String },
    Approve { request_id: String, choice: String },
    Setting { key: String, value: String },
}

/// Minimum interval between UI patches while streaming (≈ one 120 Hz frame). Deltas
/// arriving faster are coalesced, so a token flood never floods the UI thread.
const FRAME: Duration = Duration::from_millis(8);

/// Wire UI callbacks to a command channel and spawn the controller on `rt`.
pub fn start(ui: &AppWindow, rt: &tokio::runtime::Runtime) {
    let (tx, rx) = mpsc::unbounded_channel::<Cmd>();
    let st = ui.global::<AppState>();
    let t = tx.clone();
    st.on_send(move |s| {
        let s = s.trim().to_owned();
        if !s.is_empty() {
            let _ = t.send(Cmd::Send(s));
        }
    });
    let t = tx.clone();
    st.on_stop(move || {
        let _ = t.send(Cmd::Stop);
    });
    let t = tx.clone();
    st.on_new_chat(move || {
        let _ = t.send(Cmd::NewChat);
    });
    let t = tx.clone();
    st.on_open_session(move |id| {
        let _ = t.send(Cmd::Open(id.into()));
    });
    let t = tx.clone();
    st.on_delete_session(move |id| {
        let _ = t.send(Cmd::Delete(id.into()));
    });
    let t = tx.clone();
    st.on_search_sessions(move |q| {
        let _ = t.send(Cmd::Search(q.into()));
    });
    let t = tx.clone();
    st.on_refresh_models(move || {
        let _ = t.send(Cmd::RefreshModels);
    });
    let t = tx.clone();
    st.on_select_model(move |p, m| {
        let _ = t.send(Cmd::SelectModel {
            provider: p.into(),
            model: m.into(),
        });
    });
    let t = tx.clone();
    st.on_approve(move |id, choice| {
        let _ = t.send(Cmd::Approve {
            request_id: id.into(),
            choice: choice.into(),
        });
    });
    let w = ui.as_weak();
    st.on_dismiss_banner(move || {
        if let Some(ui) = w.upgrade() {
            ui.global::<AppState>().set_banner(SharedString::new());
        }
    });
    let t = tx.clone();
    let w = ui.as_weak();
    ui.global::<SettingsState>().on_changed(move |k, v| {
        // Purely visual prefs are applied immediately on the UI thread.
        if let Some(ui) = w.upgrade() {
            apply_visual_pref(&ui, k.as_str(), v.as_str());
        }
        let _ = t.send(Cmd::Setting {
            key: k.into(),
            value: v.into(),
        });
    });

    ui.global::<AppState>()
        .set_greeting(greeting_for_hour(local_hour()).into());
    ui.global::<AppState>().set_user_name(user_name().into());

    let weak = ui.as_weak();
    rt.spawn(async move {
        Controller::run(weak, rx).await;
    });
}

pub fn apply_visual_pref(ui: &AppWindow, key: &str, value: &str) {
    match key {
        "theme" => {
            let mode = match value {
                "1" => crate::ThemeMode::Light,
                "2" => crate::ThemeMode::Dark,
                _ => crate::ThemeMode::System,
            };
            ui.global::<Theme>().set_mode(mode);
        }
        "reduce_motion" => ui.global::<crate::Motion>().set_reduce(value == "true"),
        "code_font" if !value.trim().is_empty() => ui.global::<Theme>().set_mono_font(value.trim().into()),
        "ui_font" => ui
            .global::<Theme>()
            .set_ui_font(if value == "1" { "sans-serif" } else { "Inter" }.into()),
        "transcript_default" => {
            if let Ok(i) = value.parse() {
                ui.global::<AppState>().set_transcript_mode(i);
            }
        }
        _ => {}
    }
}

pub fn greeting_for_hour(h: u32) -> &'static str {
    match h {
        5..=11 => "Good morning",
        12..=17 => "Good afternoon",
        18..=21 => "Good evening",
        _ => "Up late",
    }
}

fn local_hour() -> u32 {
    use chrono::Timelike;
    chrono::Local::now().hour()
}

fn user_name() -> String {
    let raw = std::env::var("USER").unwrap_or_else(|_| "there".into());
    let first = raw.split(['-', '.', '_']).next().unwrap_or(&raw);
    let mut c = first.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => "there".into(),
    }
}

/// Run `f` on the UI thread with the window, if it still exists.
fn ui(weak: &Weak<AppWindow>, f: impl FnOnce(&AppWindow) + Send + 'static) {
    let _ = weak.upgrade_in_event_loop(move |w| f(&w));
}

struct Controller {
    weak: Weak<AppWindow>,
    client: Client,
    _backend: Arc<ManagedBackend>,
    transcript: Transcript,
    /// Runtime session id of the open chat (None = new-chat screen).
    session: Option<String>,
    /// Stored id of the open chat (sidebar highlight / delete).
    stored: Option<String>,
    all_sessions: Vec<SessionListRow>,
    query: String,
    dirty: Vec<usize>,
    reset: bool,
    pending_approval: Option<String>,
}

impl Controller {
    async fn run(weak: Weak<AppWindow>, mut cmds: mpsc::UnboundedReceiver<Cmd>) {
        ui(&weak, |w| w.global::<AppState>().set_connection("connecting".into()));
        let t0 = std::time::Instant::now();
        let backend = match backend::discover_or_spawn(None).await {
            Ok(b) => Arc::new(b),
            Err(e) => return fail(&weak, format!("Could not start the Hermes backend: {e}")),
        };
        tracing::info!(port = backend.endpoint.port, owned = backend.is_owned(), elapsed = ?t0.elapsed(), "backend ready");
        let (client, mut events) = match Client::connect(&backend.endpoint.ws_url()).await {
            Ok(c) => c,
            Err(e) => return fail(&weak, format!("Could not connect to the Hermes backend: {e}")),
        };
        let mut c = Controller {
            weak: weak.clone(),
            client,
            _backend: backend,
            transcript: Transcript::new(),
            session: None,
            stored: None,
            all_sessions: vec![],
            query: String::new(),
            dirty: vec![],
            reset: false,
            pending_approval: None,
        };
        ui(&weak, |w| {
            let st = w.global::<AppState>();
            st.set_connection("open".into());
            st.set_rows(ModelRc::new(VecModel::<ChatRow>::default()));
        });
        c.refresh_sessions().await;
        c.refresh_model_label().await;
        c.load_about().await;
        // Dev/screenshot hook: send one prompt automatically once connected.
        if let Ok(p) = std::env::var("HERMES_DESKTOP_AUTOSEND")
            && !p.trim().is_empty()
        {
            c.send(p).await;
        }

        let mut tick = tokio::time::interval(FRAME);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                cmd = cmds.recv() => match cmd {
                    Some(cmd) => c.handle_cmd(cmd).await,
                    None => break,
                },
                ev = events.recv() => match ev {
                    Some(ev) => c.handle_event(ev).await,
                    None => {
                        fail(&weak, "Lost connection to the Hermes backend.".into());
                        break;
                    }
                },
                _ = tick.tick(), if c.reset || !c.dirty.is_empty() => c.flush(),
            }
        }
    }

    /// Push changed rows to the UI (coalesced to at most one patch per frame).
    fn flush(&mut self) {
        if self.reset {
            self.reset = false;
            self.dirty.clear();
            // Blocks are Send; Slint rows (which hold Rc models) are built on the UI thread.
            let blocks = self.transcript.blocks.clone();
            let (running, status) = (self.transcript.running, self.transcript.status.clone());
            ui(&self.weak, move |w| {
                let st = w.global::<AppState>();
                let rows: Vec<ChatRow> = blocks.iter().map(to_row).collect();
                st.set_rows(ModelRc::new(VecModel::from(rows)));
                st.set_running(running);
                st.set_status(status.into());
            });
            return;
        }
        let mut idx = std::mem::take(&mut self.dirty);
        idx.sort_unstable();
        idx.dedup();
        let patch: Vec<(usize, hermes_core::transcript::Block)> = idx
            .into_iter()
            .filter_map(|i| self.transcript.blocks.get(i).map(|b| (i, b.clone())))
            .collect();
        let (running, status, title) = (
            self.transcript.running,
            self.transcript.status.clone(),
            self.transcript.title.clone(),
        );
        ui(&self.weak, move |w| {
            let st = w.global::<AppState>();
            let rows = st.get_rows();
            let Some(vm) = rows.as_any().downcast_ref::<VecModel<ChatRow>>() else {
                return;
            };
            for (i, b) in patch {
                let r = to_row(&b);
                if i < vm.row_count() {
                    vm.set_row_data(i, r);
                } else {
                    // Rows are only ever appended in order.
                    vm.push(r);
                }
            }
            st.set_running(running);
            st.set_status(status.into());
            if let Some(t) = title.filter(|t| !t.is_empty()) {
                st.set_session_title(t.into());
            }
        });
    }

    fn mark(&mut self, ch: Change) {
        match ch {
            Change::None => {}
            Change::Updated(i) => self.dirty.push(i),
            // An insert can finalise the row before it (e.g. tool.start ends a text block).
            Change::Inserted(i) => {
                if i > 0 {
                    self.dirty.push(i - 1);
                }
                self.dirty.push(i);
            }
            Change::Reset => self.reset = true,
        }
    }

    fn banner(&self, text: String, error: bool) {
        ui(&self.weak, move |w| {
            let st = w.global::<AppState>();
            st.set_banner(text.into());
            st.set_banner_error(error);
        });
    }

    async fn handle_cmd(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Send(text) => self.send(text).await,
            Cmd::Stop => {
                if let Some(sid) = self.session.clone() {
                    let _ = self
                        .client
                        .request("session.interrupt", json!({"session_id": sid}))
                        .await;
                }
            }
            Cmd::NewChat => {
                self.session = None;
                self.stored = None;
                self.transcript = Transcript::new();
                self.reset = true;
                self.flush();
                ui(&self.weak, |w| {
                    let st = w.global::<AppState>();
                    st.set_active_session(SharedString::new());
                    st.set_session_title("New chat".into());
                });
            }
            Cmd::Open(id) => self.open(id).await,
            Cmd::Delete(id) => match self.client.request("session.delete", json!({"session_id": id})).await {
                Ok(_) => {
                    if self.stored.as_deref() == Some(id.as_str()) {
                        Box::pin(self.handle_cmd(Cmd::NewChat)).await;
                    }
                    self.refresh_sessions().await;
                }
                Err(e) => self.banner(format!("Could not delete chat: {e}"), true),
            },
            Cmd::Search(q) => {
                self.query = q;
                self.publish_sessions();
            }
            Cmd::RefreshModels => self.refresh_models().await,
            Cmd::SelectModel { provider, model } => {
                let mut params = json!({"key": "model", "value": format!("{model} --provider {provider}")});
                if let Some(sid) = &self.session {
                    params["session_id"] = json!(sid);
                }
                match self.client.request("config.set", params).await {
                    Ok(_) => self.refresh_model_label().await,
                    Err(e) => self.banner(format!("Could not switch model: {e}"), true),
                }
            }
            Cmd::Approve { request_id, choice } => {
                if let Some(rpc_id) = self.pending_approval.take() {
                    self.client.respond(&rpc_id, Ok(json!({"choice": choice})));
                } else if let Some(sid) = &self.session {
                    let _ = self
                        .client
                        .request(
                            "approval.respond",
                            json!({"session_id": sid, "request_id": request_id, "choice": choice}),
                        )
                        .await;
                }
                ui(&self.weak, |w| {
                    w.global::<AppState>().set_approval(ApprovalInfo::default())
                });
            }
            Cmd::Setting { key, value } => {
                if key == "yolo"
                    && let Some(sid) = &self.session
                {
                    let v = if value == "true" { "on" } else { "off" };
                    let _ = self
                        .client
                        .request(
                            "config.set",
                            json!({"key": "yolo", "value": v, "session_id": sid, "scope": "session"}),
                        )
                        .await;
                }
            }
        }
    }

    async fn ensure_session(&mut self) -> Option<String> {
        if let Some(s) = &self.session {
            return Some(s.clone());
        }
        match self
            .client
            .call::<SessionCreateResult>("session.create", json!({"source": "desktop"}))
            .await
        {
            Ok(r) => {
                self.session = Some(r.session_id.clone());
                self.stored = Some(r.stored_session_id.clone()).filter(|s| !s.is_empty());
                let stored = self.stored.clone().unwrap_or_default();
                let info = r.info;
                ui(&self.weak, move |w| {
                    let st = w.global::<AppState>();
                    st.set_active_session(stored.into());
                    st.set_model_label(short_model(&info.model).into());
                    st.set_effort_label(info.reasoning_effort.into());
                    st.set_cwd_label(short_path(&info.cwd).into());
                });
                Some(r.session_id)
            }
            Err(e) => {
                self.banner(format!("Could not start a chat: {e}"), true);
                None
            }
        }
    }

    async fn send(&mut self, text: String) {
        let ch = self.transcript.push_user(&text);
        self.mark(ch);
        self.flush();
        let Some(sid) = self.ensure_session().await else {
            self.transcript.running = false;
            self.flush_state();
            return;
        };
        let client = self.client.clone();
        let weak = self.weak.clone();
        // prompt.submit acks immediately; streaming arrives as events. Don't block the loop.
        tokio::spawn(async move {
            if let Err(e) = client
                .request_with_timeout(
                    "prompt.submit",
                    json!({"session_id": sid, "text": text}),
                    Duration::from_secs(1800),
                )
                .await
            {
                ui(&weak, move |w| {
                    let st = w.global::<AppState>();
                    st.set_banner(format!("Message failed: {e}").into());
                    st.set_banner_error(true);
                    st.set_running(false);
                });
            }
        });
    }

    fn flush_state(&self) {
        let (running, status) = (self.transcript.running, self.transcript.status.clone());
        ui(&self.weak, move |w| {
            let st = w.global::<AppState>();
            st.set_running(running);
            st.set_status(status.into());
        });
    }

    async fn open(&mut self, stored_id: String) {
        if self.stored.as_deref() == Some(stored_id.as_str()) {
            return;
        }
        match self
            .client
            .call::<SessionResumeResult>("session.resume", json!({"session_id": stored_id, "source": "desktop"}))
            .await
        {
            Ok(r) => {
                self.session = Some(r.session_id.clone());
                self.stored = Some(stored_id.clone());
                self.transcript = Transcript::new();
                let ch = self.transcript.load_history(&r.messages);
                self.transcript.running = r.running.unwrap_or(r.info.running);
                self.transcript.title = Some(r.info.title.clone()).filter(|t| !t.is_empty());
                self.mark(ch);
                self.flush();
                let info = r.info;
                let title = self
                    .all_sessions
                    .iter()
                    .find(|s| s.id == stored_id)
                    .map(display_title)
                    .unwrap_or_else(|| info.title.clone());
                ui(&self.weak, move |w| {
                    let st = w.global::<AppState>();
                    st.set_active_session(stored_id.into());
                    st.set_session_title(title.into());
                    st.set_model_label(short_model(&info.model).into());
                    st.set_effort_label(info.reasoning_effort.into());
                    st.set_cwd_label(short_path(&info.cwd).into());
                });
            }
            Err(e) => self.banner(format!("Could not open chat: {e}"), true),
        }
    }

    async fn handle_event(&mut self, ev: ClientEvent) {
        match ev {
            ClientEvent::Connected => {}
            ClientEvent::Disconnected(why) => fail(&self.weak, format!("Disconnected from backend: {why}")),
            ClientEvent::ServerRequest { id, method, params } => self.server_request(id, method, params),
            ClientEvent::Event(e) => {
                if e.kind == "sessions.changed" {
                    self.refresh_sessions().await;
                    return;
                }
                // Only the open session's events touch the transcript.
                if e.session_id.is_some() && e.session_id != self.session {
                    return;
                }
                let ch = self.transcript.apply(&e);
                self.mark(ch);
                if matches!(
                    e.kind.as_str(),
                    "thinking.delta" | "status.update" | "tool.generating" | "message.start"
                ) {
                    // Status-only events still need a (coalesced) repaint of the status line.
                    self.flush_state();
                }
                if e.kind == "message.complete" {
                    self.flush();
                }
            }
        }
    }

    fn server_request(&mut self, id: String, method: String, params: Value) {
        match method.as_str() {
            "approval" => {
                let s = |k: &str| params.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
                self.pending_approval = Some(id);
                let info = ApprovalInfo {
                    visible: true,
                    request_id: s("request_id").into(),
                    title: "Allow this command?".into(),
                    command: s("command").into(),
                    description: s("description").into(),
                };
                ui(&self.weak, move |w| w.global::<AppState>().set_approval(info));
            }
            other => {
                // Not yet implemented in this build (clarify, secret, sudo, preview.*): decline
                // explicitly so the agent never hangs waiting. Tracked in docs/PARITY.md.
                tracing::warn!(method = other, "declining unsupported server request");
                self.client.respond(
                    &id,
                    Err((-32601, format!("{other} is not supported by this client yet"))),
                );
                self.banner(
                    format!("The agent asked for '{other}', which this build can't answer yet."),
                    false,
                );
            }
        }
    }

    async fn refresh_sessions(&mut self) {
        match self
            .client
            .call::<SessionListResult>("session.list", json!({"limit": 200}))
            .await
        {
            Ok(r) => {
                self.all_sessions = r.sessions;
                self.publish_sessions();
            }
            Err(e) => tracing::warn!("session.list failed: {e}"),
        }
    }

    fn publish_sessions(&self) {
        let q = self.query.to_lowercase();
        let rows: Vec<SessionListRow> = self
            .all_sessions
            .iter()
            .filter(|r| q.is_empty() || display_title(r).to_lowercase().contains(&q))
            .cloned()
            .collect();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or_default();
        let mut items = Vec::with_capacity(rows.len());
        for g in group_by_age(&rows, now) {
            for (k, &i) in g.indices.iter().enumerate() {
                let r = &rows[i];
                items.push(SessionItem {
                    id: r.id.as_str().into(),
                    title: display_title(r).into(),
                    group: g.label.as_str().into(),
                    first_in_group: k == 0,
                    pinned: false,
                    running: false,
                });
            }
        }
        ui(&self.weak, move |w| {
            w.global::<AppState>()
                .set_sessions(Rc::new(VecModel::from(items)).into());
        });
    }

    async fn refresh_model_label(&self) {
        let mut params = json!({});
        if let Some(sid) = &self.session {
            params["session_id"] = json!(sid);
        }
        if let Ok(r) = self.client.call::<ModelOptionsResult>("model.options", params).await {
            let label = short_model(&r.model);
            ui(&self.weak, move |w| {
                w.global::<AppState>().set_model_label(label.into())
            });
        }
    }

    async fn refresh_models(&self) {
        let mut params = json!({});
        if let Some(sid) = &self.session {
            params["session_id"] = json!(sid);
        }
        match self.client.call::<ModelOptionsResult>("model.options", params).await {
            Ok(r) => {
                let mut items = Vec::new();
                for p in &r.providers {
                    let ids = p.model_ids();
                    if ids.is_empty() {
                        continue;
                    }
                    items.push(ModelItem {
                        provider: p.slug.as_str().into(),
                        provider_name: p.name.as_str().into(),
                        header: true,
                        ..Default::default()
                    });
                    for m in ids {
                        let current = p.slug == r.provider && m == r.model;
                        items.push(ModelItem {
                            provider: p.slug.as_str().into(),
                            provider_name: p.name.as_str().into(),
                            model: m.into(),
                            current,
                            header: false,
                        });
                    }
                }
                ui(&self.weak, move |w| {
                    w.global::<AppState>().set_models(Rc::new(VecModel::from(items)).into())
                });
            }
            Err(e) => self.banner(format!("Could not load models: {e}"), true),
        }
    }

    async fn load_about(&self) {
        let version = self
            .client
            .request("config.get", json!({"key": "version"}))
            .await
            .ok()
            .and_then(|v| v.get("value").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_else(|| "connected".into());
        let home = std::env::var("HERMES_HOME")
            .unwrap_or_else(|_| format!("{}/.hermes", std::env::var("HOME").unwrap_or_default()));
        ui(&self.weak, move |w| {
            let s = w.global::<SettingsState>();
            s.set_backend_version(version.into());
            s.set_hermes_home(home.into());
        });
    }
}

fn fail(weak: &Weak<AppWindow>, msg: String) {
    tracing::error!("{msg}");
    ui(weak, move |w| {
        let st = w.global::<AppState>();
        st.set_connection("error".into());
        st.set_connection_detail(msg.as_str().into());
        st.set_banner(msg.into());
        st.set_banner_error(true);
        st.set_running(false);
    });
}

/// `deepseek/deepseek-v4.1-flash` → `deepseek-v4.1-flash`.
pub fn short_model(m: &str) -> String {
    m.rsplit('/').next().unwrap_or(m).to_owned()
}

/// `/home/u/Projects/x` → `x`.
pub fn short_path(p: &str) -> String {
    std::path::Path::new(p)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(short_model("deepseek/deepseek-v4.1-flash"), "deepseek-v4.1-flash");
        assert_eq!(short_model("gpt-5"), "gpt-5");
        assert_eq!(short_path("/home/u/Projects/x"), "x");
        assert_eq!(greeting_for_hour(2), "Up late");
        assert_eq!(greeting_for_hour(9), "Good morning");
        assert_eq!(greeting_for_hour(15), "Good afternoon");
        assert_eq!(greeting_for_hour(20), "Good evening");
    }
}
