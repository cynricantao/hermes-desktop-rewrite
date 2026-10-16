//! Async controller: owns backend + client + transcript; talks to the UI only through
//! `invoke_from_event_loop` (UI ← core) and an unbounded command channel (UI → core).

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use hermes_core::backend::{self, ManagedBackend};
use hermes_core::http::{Rest, profile_query};
use hermes_core::prefs::Prefs;
use hermes_core::protocol::{
    ModelOptionsResult, SessionCreateResult, SessionListResult, SessionListRow, SessionResumeResult,
};
use hermes_core::sessions::{cycle_index, display_title, group_by_age};
use hermes_core::settings::{self, Store};
use hermes_core::transcript::{Change, Transcript};
use hermes_core::{Client, ClientEvent};
use serde_json::{Value, json};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel, Weak};
use tokio::sync::mpsc;

use crate::rows::to_row;
use crate::{
    AppState, AppWindow, ApprovalInfo, ChatRow, ModelItem, NavEntry, ScopeChip, SearchHit, SessionItem, SettingRowUI,
    SettingsState, Theme,
};

/// Commands from the UI thread to the controller.
#[derive(Debug, Clone)]
pub enum Cmd {
    Send(String),
    Stop,
    NewChat,
    Open(String),
    Delete(String),
    Search(String),
    /// Open the chat this many places away in sidebar order (Ctrl+Tab = 1, Ctrl+Shift+Tab = -1).
    Cycle(i32),
    RefreshModels,
    SelectModel {
        provider: String,
        model: String,
    },
    Approve {
        request_id: String,
        choice: String,
    },
    Settings(SettingsCmd),
}

/// Settings-panel intents from the UI (see `ui/settings.slint`).
#[derive(Debug, Clone)]
pub enum SettingsCmd {
    /// The panel opened (Ctrl+, or the dev hook).
    Opened,
    Closed,
    /// Navigate to a page. An empty `page` means the group's first page.
    Goto {
        group: String,
        page: String,
    },
    Search(String),
    /// Navigate to the page that holds a search hit and highlight the row.
    Jump {
        group: String,
        page: String,
        setting: String,
    },
    /// "Applies to": switch the profile whose config the page reads and writes.
    Scope(String),
    /// One edited control, with its value already in the control's own text form.
    Edit {
        id: String,
        value: String,
    },
    /// A button row (`Control::Action`).
    Action(String),
    Export,
    Import,
    Reset,
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
    st.on_cycle_session(move |step| {
        let _ = t.send(Cmd::Cycle(step));
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
    let s = ui.global::<SettingsState>();
    s.on_opened(move || {
        let _ = t.send(Cmd::Settings(SettingsCmd::Opened));
    });
    let t = tx.clone();
    s.on_closed(move || {
        let _ = t.send(Cmd::Settings(SettingsCmd::Closed));
    });
    let t = tx.clone();
    s.on_goto(move |g, p| {
        let _ = t.send(Cmd::Settings(SettingsCmd::Goto {
            group: g.into(),
            page: p.into(),
        }));
    });
    let t = tx.clone();
    s.on_search(move |q| {
        let _ = t.send(Cmd::Settings(SettingsCmd::Search(q.into())));
    });
    let t = tx.clone();
    s.on_jump(move |g, p, id| {
        let _ = t.send(Cmd::Settings(SettingsCmd::Jump {
            group: g.into(),
            page: p.into(),
            setting: id.into(),
        }));
    });
    let t = tx.clone();
    s.on_scope(move |name| {
        let _ = t.send(Cmd::Settings(SettingsCmd::Scope(name.into())));
    });
    let t = tx.clone();
    let w = ui.as_weak();
    s.on_edit(move |id, value| {
        // Device prefs that change the look apply immediately on the UI thread; the
        // persistent write goes through the controller like every other setting.
        if let Some(ui) = w.upgrade() {
            apply_device_pref(&ui, id.as_str(), value.as_str());
        }
        let _ = t.send(Cmd::Settings(SettingsCmd::Edit {
            id: id.into(),
            value: value.into(),
        }));
    });
    let t = tx.clone();
    s.on_action(move |id| {
        let _ = t.send(Cmd::Settings(SettingsCmd::Action(id.into())));
    });
    let t = tx.clone();
    s.on_export_config(move || {
        let _ = t.send(Cmd::Settings(SettingsCmd::Export));
    });
    let t = tx.clone();
    s.on_import_config(move || {
        let _ = t.send(Cmd::Settings(SettingsCmd::Import));
    });
    let t = tx.clone();
    s.on_reset_config(move || {
        let _ = t.send(Cmd::Settings(SettingsCmd::Reset));
    });

    ui.global::<AppState>()
        .set_greeting(greeting_for_hour(local_hour()).into());
    ui.global::<AppState>().set_user_name(user_name().into());

    let weak = ui.as_weak();
    rt.spawn(async move {
        Controller::run(weak, rx).await;
    });
}

/// Device preferences that change the look *now*, applied on the UI thread the moment a
/// control moves. Everything else is only stored (the row's feature reads it when built),
/// and every device pref is persisted by the controller in `prefs.json`.
pub fn apply_device_pref(ui: &AppWindow, key: &str, value: &str) {
    // Appearance › Theme mode (the original's `hermes-desktop-mode-v1`).
    if key == "hermes-desktop-mode-v1" {
        let mode = match value {
            "light" => crate::ThemeMode::Light,
            "dark" => crate::ThemeMode::Dark,
            _ => crate::ThemeMode::System,
        };
        ui.global::<Theme>().set_mode(mode);
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
    /// Stored ids in the order the sidebar shows them (after search + grouping).
    visible_ids: Vec<String>,
    query: String,
    dirty: Vec<usize>,
    reset: bool,
    pending_approval: Option<String>,
    /// Model picked on the new-chat screen before a session exists: (provider, model).
    /// Sent with `session.create` (as the Electron app does); `config.set model` needs a
    /// live session and answers 4001 without one.
    draft_model: Option<(String, String)>,
    /// REST handle to the backend's web API (settings pages). Built on first use.
    rest: Option<Rest>,
    /// `GET /api/profiles` roster: (name, label) in backend order.
    profiles: Vec<(String, String)>,
    /// Profile whose config the settings pages read and write ("" = the default profile).
    scope: String,
    /// Config of `scope`, as loaded. Also the local copy edits merge into.
    config: Value,
    /// `/api/config/schema` fields, for runtime option lists (timezones, providers).
    schema: Option<Value>,
    /// Device preferences (the original's Electron-storage settings).
    prefs: Prefs,
    /// Open settings page: (group, page).
    settings_page: (String, String),
    /// Row id a search jumped to (highlighted on its page).
    settings_hit: String,
    /// Set while a Reset click is waiting for its confirmation click.
    reset_armed: bool,
}

impl Controller {
    async fn run(weak: Weak<AppWindow>, mut cmds: mpsc::UnboundedReceiver<Cmd>) {
        ui(&weak, |w| w.global::<AppState>().set_connection("connecting".into()));
        crate::perf::mark("controller: backend discovery start");
        let backend = match backend::discover_or_spawn(None).await {
            Ok(b) => Arc::new(b),
            Err(e) => return fail(&weak, format!("Could not start the Hermes backend: {e}")),
        };
        tracing::info!(
            port = backend.endpoint.port,
            owned = backend.is_owned(),
            "backend ready"
        );
        crate::perf::mark("controller: backend discovered (token fetched)");
        let (client, mut events) = match Client::connect(&backend.endpoint.ws_url()).await {
            Ok(c) => c,
            Err(e) => return fail(&weak, format!("Could not connect to the Hermes backend: {e}")),
        };
        crate::perf::mark("controller: websocket connected");
        // Tell the backend this client answers server→client requests (approval, …). Without
        // it the backend treats us as an old build and withdraws every approval unseen
        // (tui_gateway/server_requests.py). Unsupported methods are still answered with -32601.
        if let Err(e) = client
            .request("client.capabilities", json!({"server_requests": true}))
            .await
        {
            tracing::warn!("client.capabilities failed: {e}");
        }
        let mut c = Controller {
            weak: weak.clone(),
            client,
            _backend: backend,
            transcript: Transcript::new(),
            session: None,
            stored: None,
            draft_model: None,
            all_sessions: vec![],
            visible_ids: vec![],
            query: String::new(),
            dirty: vec![],
            reset: false,
            pending_approval: None,
            rest: None,
            profiles: Vec::new(),
            scope: String::new(),
            config: Value::Null,
            schema: None,
            prefs: Prefs::load(hermes_core::prefs::default_path()),
            settings_page: ("appearance".into(), "general".into()),
            settings_hit: String::new(),
            reset_armed: false,
        };
        ui(&weak, |w| {
            let st = w.global::<AppState>();
            st.set_connection("open".into());
            st.set_rows(ModelRc::new(VecModel::<ChatRow>::default()));
        });
        c.refresh_sessions().await;
        crate::perf::mark("controller: session list fetched");
        c.refresh_model_label().await;
        crate::perf::mark("controller: model label fetched");
        c.load_about().await;
        // Dev/measurement hook: open a stored session once connected.
        if let Ok(id) = std::env::var("HERMES_DESKTOP_OPEN_SESSION")
            && !id.trim().is_empty()
        {
            c.open(id.trim().to_owned()).await;
        }
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
                self.draft_model = None;
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
            Cmd::Cycle(step) => {
                if let Some(i) = cycle_index(&self.visible_ids, self.stored.as_deref(), step) {
                    let id = self.visible_ids[i].clone();
                    self.open(id).await;
                }
            }
            Cmd::RefreshModels => self.refresh_models().await,
            Cmd::SelectModel { provider, model } => {
                let Some(sid) = self.session.clone() else {
                    // New-chat screen: remember the pick; it rides on session.create.
                    let label = short_model(&model);
                    self.draft_model = Some((provider, model));
                    ui(&self.weak, move |w| {
                        w.global::<AppState>().set_model_label(label.into())
                    });
                    return;
                };
                let params = json!({
                    "key": "model",
                    "value": format!("{model} --provider {provider}"),
                    "session_id": sid,
                });
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
            Cmd::Settings(cmd) => self.settings_cmd(cmd).await,
        }
    }

    // ── Settings panel ──────────────────────────────────────────────────────────

    /// REST handle for the settings pages, built from the backend we already own.
    fn rest(&mut self) -> Option<Rest> {
        if self.rest.is_none() {
            let e = &self._backend.endpoint;
            self.rest = Some(Rest::new(e.host.clone(), e.port, e.token.clone()));
        }
        self.rest.clone()
    }

    /// Open the panel: load the profile roster, the scoped config and the schema, then
    /// render the current page.
    async fn settings_open(&mut self) {
        let Some(rest) = self.rest() else { return };
        self.set_loading(true);
        if self.profiles.is_empty() {
            match rest.get("/api/profiles").await {
                Ok(v) => self.profiles = parse_profiles(&v),
                Err(e) => {
                    return self.settings_error(format!("Could not load profiles: {e}"));
                }
            }
        }
        if self.schema.is_none() {
            match rest.get("/api/config/schema").await {
                Ok(v) => self.schema = Some(v),
                Err(e) => tracing::warn!("config schema unavailable: {e}"),
            }
        }
        self.load_scoped_config().await;
        self.publish_settings();
        self.set_loading(false);
    }

    /// Load (or reload) the config of the scoped profile.
    async fn load_scoped_config(&mut self) {
        let Some(rest) = self.rest() else { return };
        let path = format!("/api/config{}", profile_query(&self.scope));
        match rest.get(&path).await {
            Ok(v) => self.config = v,
            Err(e) => {
                self.config = Value::Object(Default::default());
                self.settings_error(format!("Could not load this profile's config: {e}"));
            }
        }
    }

    async fn settings_cmd(&mut self, cmd: SettingsCmd) {
        match cmd {
            SettingsCmd::Opened => self.settings_open().await,
            SettingsCmd::Closed => {
                self.settings_hit.clear();
            }
            SettingsCmd::Goto { group, page } => {
                let (g, p) = resolve_page(&group, &page);
                self.settings_page = (g, p);
                self.settings_hit.clear();
                self.publish_settings();
            }
            SettingsCmd::Search(q) => self.publish_hits(&q),
            SettingsCmd::Jump { group, page, setting } => {
                let (g, p) = resolve_page(&group, &page);
                self.settings_page = (g, p);
                self.settings_hit = setting;
                self.publish_settings();
            }
            SettingsCmd::Scope(name) => {
                if name != self.scope {
                    self.scope = name;
                    self.load_scoped_config().await;
                    self.publish_settings();
                }
            }
            SettingsCmd::Edit { id, value } => self.edit_setting(&id, &value).await,
            SettingsCmd::Action(id) => self.settings_action(&id).await,
            SettingsCmd::Export => self.export_config().await,
            SettingsCmd::Import => self.import_config().await,
            SettingsCmd::Reset => self.reset_config().await,
        }
    }

    /// Persist one edited control and re-render its page.
    async fn edit_setting(&mut self, id: &str, value: &str) {
        let Some(s) = settings::setting(id) else {
            return;
        };
        let current = match s.store {
            Store::Config(key) => settings::get(&self.config, key),
            Store::Device(key) => self.prefs.get(key),
            Store::None => None,
        };
        let parsed = match settings::parse_value(&s.control, value, current) {
            Ok(v) => normalize_number(v),
            Err(e) => return self.settings_error(e),
        };
        match s.store {
            Store::Config(key) => {
                let Some(rest) = self.rest() else { return };
                let patch = settings::sparse_patch(key, parsed.clone());
                let path = format!("/api/config{}", profile_query(&self.scope));
                let body = json!({ "config": patch });
                match rest.put(&path, &body).await {
                    Ok(_) => {
                        // Keep the local copy in step so the page shows what was saved.
                        settings::set_in(&mut self.config, key, parsed.clone());
                        self.settings_saved(key);
                        // Session-scoped switches the backend also tracks per session.
                        if key == "yolo"
                            && let Some(sid) = &self.session
                        {
                            let v = if parsed == Value::Bool(true) { "on" } else { "off" };
                            let _ = self
                                .client
                                .request(
                                    "config.set",
                                    json!({"key": "yolo", "value": v, "session_id": sid, "scope": "session"}),
                                )
                                .await;
                        }
                    }
                    Err(e) => self.settings_error(format!("Could not save {key}: {e}")),
                }
            }
            Store::Device(key) => {
                if let Err(e) = self.prefs.set(key, parsed) {
                    return self.settings_error(format!("Could not save {key}: {e}"));
                }
                self.settings_saved(key);
            }
            Store::None => {}
        }
        self.publish_settings();
    }

    /// `Send test notification`-style rows.
    async fn settings_action(&mut self, id: &str) {
        match id {
            "test-notification" => {
                let Some(rest) = self.rest() else { return };
                let body = json!({"title": "Hermes", "body": "Test notification"});
                match rest.post("/api/desktop/notify", &body).await {
                    Ok(_) => self.settings_notice("Test notification sent", false),
                    Err(e) => self.settings_error(format!("Could not send a notification: {e}")),
                }
            }
            other => self.settings_error(format!("No handler for the “{other}” action yet")),
        }
    }

    /// Export the scoped profile's config to a JSON file under the app's data directory
    /// (the original downloads it; we have no file dialog yet — see PARITY.md).
    async fn export_config(&mut self) {
        let Some(rest) = self.rest() else { return };
        let path = format!("/api/config{}", profile_query(&self.scope));
        let cfg = match rest.get(&path).await {
            Ok(v) => v,
            Err(e) => return self.settings_error(format!("Could not read the config: {e}")),
        };
        let dir = export_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            return self.settings_error(format!("Could not create {}: {e}", dir.display()));
        }
        let name = if self.scope.is_empty() {
            "hermes-config.json".to_owned()
        } else {
            format!("hermes-config-{}.json", self.scope)
        };
        let file = dir.join(name);
        let body = serde_json::to_vec_pretty(&cfg).unwrap_or_default();
        match std::fs::write(&file, body) {
            Ok(()) => self.settings_notice(format!("Exported to {}", file.display()), false),
            Err(e) => self.settings_error(format!("Could not write {}: {e}", file.display())),
        }
    }

    /// Import the newest `*.json` a user dropped into the import directory and PUT it as
    /// the scoped profile's config.
    async fn import_config(&mut self) {
        let dir = import_dir();
        let file = std::fs::read_dir(&dir).ok().and_then(|rd| {
            let mut files: Vec<_> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "json"))
                .collect();
            files.sort_by_key(|p| {
                std::fs::metadata(p)
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
            });
            files.pop()
        });
        let Some(file) = file else {
            return self.settings_error(format!(
                "No config to import: drop a hermes-config.json into {} and press Import again",
                dir.display()
            ));
        };
        let cfg: Value = match std::fs::read(&file).ok().and_then(|b| serde_json::from_slice(&b).ok()) {
            Some(v) => v,
            None => return self.settings_error(format!("{} is not valid JSON", file.display())),
        };
        let Some(rest) = self.rest() else { return };
        let path = format!("/api/config{}", profile_query(&self.scope));
        match rest.put(&path, &json!({ "config": cfg })).await {
            Ok(_) => {
                self.load_scoped_config().await;
                self.settings_notice(format!("Imported {}", file.display()), false);
            }
            Err(e) => self.settings_error(format!("Import failed: {e}")),
        }
        self.publish_settings();
    }

    /// Restore the backend's defaults for the scoped profile. Two clicks: the first arms
    /// it (the original asks for confirmation in a dialog).
    async fn reset_config(&mut self) {
        if !self.reset_armed {
            self.reset_armed = true;
            return self.settings_notice("Press reset again to restore this profile's Hermes defaults", true);
        }
        self.reset_armed = false;
        let Some(rest) = self.rest() else { return };
        let defaults = match rest.get("/api/config/defaults").await {
            Ok(v) => v,
            Err(e) => return self.settings_error(format!("Could not read the defaults: {e}")),
        };
        let path = format!("/api/config{}", profile_query(&self.scope));
        match rest.put(&path, &json!({ "config": defaults })).await {
            Ok(_) => {
                self.load_scoped_config().await;
                self.settings_notice("Restored Hermes defaults", false);
            }
            Err(e) => self.settings_error(format!("Reset failed: {e}")),
        }
        self.publish_settings();
    }

    // ── Rendering the panel ─────────────────────────────────────────────────────

    fn set_loading(&self, on: bool) {
        ui(&self.weak, move |w| {
            w.global::<SettingsState>().set_loading(on);
        });
    }

    fn settings_notice(&self, text: impl Into<String>, error: bool) {
        let text = text.into();
        ui(&self.weak, move |w| {
            let s = w.global::<SettingsState>();
            s.set_notice(text.into());
            s.set_notice_error(error);
        });
    }

    fn settings_saved(&self, key: &str) {
        self.settings_notice(format!("Saved {key}"), false);
    }

    fn settings_error(&self, text: String) {
        tracing::warn!("settings: {text}");
        self.settings_notice(text, true);
    }

    /// Push the nav, the page header, the "Applies to" pills and the current page's rows.
    fn publish_settings(&self) {
        let (group, page) = self.settings_page.clone();
        let nav = build_nav(&group, &page);
        let scopes = build_scopes(&self.profiles, &self.scope);
        let per_profile = settings::page(&group, &page).map(|p| p.per_profile).unwrap_or(false);
        let group_title = settings::group(&group).map(|g| g.label).unwrap_or_default();
        let page_title = settings::page(&group, &page).map(|p| p.label).unwrap_or_default();
        let schema = self.schema.clone();
        let config = self.config.clone();
        let prefs = self.prefs.all().clone();
        let hit = self.settings_hit.clone();
        let scroll = scroll_offset(&group, &page, &hit, &config, &prefs);
        ui(&self.weak, move |w| {
            let s = w.global::<SettingsState>();
            s.set_nav(ModelRc::new(VecModel::from(nav)));
            s.set_scopes(ModelRc::new(VecModel::from(scopes)));
            s.set_group(group.clone().into());
            s.set_page(page.as_str().into());
            s.set_group_title(group_title.into());
            s.set_page_title(page_title.into());
            s.set_per_profile(per_profile);
            s.set_jump_scroll(scroll);
            if !hit.is_empty() {
                // Jumping from search returns to the page view.
                s.set_query(SharedString::new());
                s.set_hits(ModelRc::new(VecModel::from(Vec::<SearchHit>::new())));
            }
            let rows = build_rows(&group, &page, &config, &prefs, schema.as_ref(), &hit);
            s.set_rows(ModelRc::new(VecModel::from(rows)));
        });
    }

    fn publish_hits(&self, query: &str) {
        let hits: Vec<SearchHit> = if query.trim().is_empty() {
            Vec::new()
        } else {
            settings::search(query, 12)
                .into_iter()
                .map(|h| SearchHit {
                    group: h.group.into(),
                    page: h.page.into(),
                    setting: h.id.into(),
                    label: h.label.into(),
                    path: h.path.into(),
                })
                .collect()
        };
        ui(&self.weak, move |w| {
            w.global::<SettingsState>().set_hits(ModelRc::new(VecModel::from(hits)));
        });
    }

    async fn ensure_session(&mut self) -> Option<String> {
        if let Some(s) = &self.session {
            return Some(s.clone());
        }
        match self
            .client
            .call::<SessionCreateResult>("session.create", {
                let mut p = json!({"source": "desktop"});
                if let Some((provider, model)) = self.draft_model.take() {
                    p["model"] = json!(model);
                    p["provider"] = json!(provider);
                }
                p
            })
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
                // The backend withdraws an open server request (timeout, Stop, another surface
                // answered): tear the card down, or it would outlive the request.
                if e.kind == "request.cancel" {
                    let id = e.payload.get("id").and_then(Value::as_str).unwrap_or_default();
                    if self.pending_approval.as_deref() == Some(id) {
                        self.pending_approval = None;
                        ui(&self.weak, |w| {
                            w.global::<AppState>().set_approval(ApprovalInfo::default())
                        });
                    }
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
                let choices = approval_choices(&params);
                let info = ApprovalInfo {
                    visible: true,
                    request_id: s("request_id").into(),
                    title: "Allow this command?".into(),
                    command: s("command").into(),
                    description: s("description").into(),
                    allow_session: choices.iter().any(|c| c == "session"),
                    allow_always: choices.iter().any(|c| c == "always"),
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

    fn publish_sessions(&mut self) {
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
        self.visible_ids = items.iter().map(|s| s.id.to_string()).collect();
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
                        let current = match (&self.session, &self.draft_model) {
                            (None, Some((dp, dm))) => &p.slug == dp && &m == dm,
                            _ => p.slug == r.provider && m == r.model,
                        };
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
            s.set_version(version.into());
            s.set_home(home.into());
        });
    }
}

/// Resolve a (group, page) pair from the UI: an empty page means the group's first page,
/// an unknown group falls back to Appearance.
fn resolve_page(group: &str, page: &str) -> (String, String) {
    let g = settings::group(group).or_else(|| settings::group("appearance"));
    let Some(g) = g else {
        return (group.to_owned(), page.to_owned());
    };
    let p = if page.is_empty() {
        g.pages.first()
    } else {
        settings::page(g.id, page).or_else(|| g.pages.first())
    };
    match p {
        Some(p) => (g.id.to_owned(), p.id.to_owned()),
        None => (g.id.to_owned(), String::new()),
    }
}

/// The nav model: every group in registry order, with the active group's pages expanded.
/// A single-page group is one row that opens its page (like the original's childless
/// groups); `divider` marks the original's nav sections.
fn build_nav(active_group: &str, active_page: &str) -> Vec<NavEntry> {
    let mut out = Vec::new();
    let mut section: Option<u8> = None;
    for g in settings::GROUPS {
        let divider = section.is_some_and(|s| s != g.section);
        section = Some(g.section);
        if g.pages.len() == 1 {
            let p = &g.pages[0];
            out.push(NavEntry {
                group: g.id.into(),
                page: p.id.into(),
                label: g.label.into(),
                icon: g.icon.into(),
                level: 0,
                divider,
                active: g.id == active_group,
                expanded: false,
            });
            continue;
        }
        let expanded = g.id == active_group;
        out.push(NavEntry {
            group: g.id.into(),
            page: SharedString::new(),
            label: g.label.into(),
            icon: g.icon.into(),
            level: 0,
            divider,
            active: expanded,
            expanded,
        });
        if expanded {
            for p in g.pages {
                out.push(NavEntry {
                    group: g.id.into(),
                    page: p.id.into(),
                    label: p.label.into(),
                    icon: p.icon.into(),
                    level: 1,
                    divider: false,
                    active: g.id == active_group && p.id == active_page,
                    expanded: false,
                });
            }
        }
    }
    out
}

/// The "Applies to" pills: every profile, with the default profile selected while the
/// scope is unset.
fn build_scopes(profiles: &[(String, String)], scope: &str) -> Vec<ScopeChip> {
    profiles
        .iter()
        .map(|(name, label)| ScopeChip {
            name: name.as_str().into(),
            label: label.as_str().into(),
            active: if scope.is_empty() {
                name == "default"
            } else {
                name == scope
            },
        })
        .collect()
}

/// `GET /api/profiles` → (name, label). The label is the bot title, else the display name,
/// else the slug — the same presentation order the original's settings chip uses.
fn parse_profiles(v: &Value) -> Vec<(String, String)> {
    v.get("profiles")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|p| {
                    let name = p.get("name")?.as_str()?.to_owned();
                    let label = ["bot_title", "display_name"]
                        .iter()
                        .filter_map(|k| p.get(*k).and_then(Value::as_str))
                        .find(|s| !s.trim().is_empty())
                        .map(str::to_owned)
                        .unwrap_or_else(|| {
                            if name == "default" {
                                "Default".to_owned()
                            } else {
                                name.clone()
                            }
                        });
                    Some((name, label))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Runtime option lists for `Control::DynSelect`, straight from `/api/config/schema`.
fn dyn_options(schema: Option<&Value>, key: &str) -> Option<Vec<String>> {
    schema?
        .get("fields")?
        .get(key)?
        .get("options")?
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect())
}

/// Widget rows carry a short note for the control that is not built yet, so the page shows
/// an honest placeholder instead of a dead control.
fn widget_note(id: &str) -> &'static str {
    match id {
        "hermes-desktop-theme-v2" => "Palette picker not built yet",
        _ => "",
    }
}

/// One page's rows, ready for the UI model.
fn build_rows(
    group: &str,
    page: &str,
    config: &Value,
    prefs: &serde_json::Map<String, Value>,
    schema: Option<&Value>,
    hit: &str,
) -> Vec<SettingRowUI> {
    settings::rows_for(group, page, config, prefs, |key| dyn_options(schema, key))
        .into_iter()
        .map(|r| SettingRowUI {
            id: r.id.into(),
            kind: r.kind.into(),
            label: r.label.into(),
            description: r.description.into(),
            value: r.value.into(),
            option_labels: ModelRc::new(VecModel::from(
                r.options
                    .iter()
                    .map(|(_, label)| SharedString::from(label.as_str()))
                    .collect::<Vec<_>>(),
            )),
            option_values: ModelRc::new(VecModel::from(
                r.options
                    .iter()
                    .map(|(value, _)| SharedString::from(value.as_str()))
                    .collect::<Vec<_>>(),
            )),
            selected: r.selected,
            min: r.min,
            max: r.max,
            unit: r.unit.into(),
            highlight: !hit.is_empty() && r.id == hit,
            note: widget_note(r.id).into(),
        })
        .collect()
}

/// Rough content offset of a search-jumped row: rows are label+description (≈76 px) or
/// label only (≈52 px) tall. The row is highlighted as well, so an imprecise offset is a
/// convenience, not a correctness problem.
fn scroll_offset(group: &str, page: &str, hit: &str, config: &Value, prefs: &serde_json::Map<String, Value>) -> i32 {
    if hit.is_empty() {
        return 0;
    }
    let mut y = 0;
    for r in settings::rows_for(group, page, config, prefs, |_| None) {
        if r.id == hit {
            return y;
        }
        y += if r.description.is_empty() { 52 } else { 76 };
    }
    0
}

/// A whole-number float (a slider's "42.0") is stored as an integer so the row reads "42".
fn normalize_number(v: Value) -> Value {
    match v {
        Value::Number(n) if n.as_i64().is_none() => match n.as_f64() {
            Some(f) if f.fract() == 0.0 && f.abs() < 9e15 => json!(f as i64),
            _ => Value::Number(n),
        },
        other => other,
    }
}

/// Where Export writes and Import looks for a config file. The original opens a native
/// save/open dialog; we have no file dialog yet, so both use a fixed directory
/// (`HERMES_DESKTOP_EXPORT_DIR` overrides it) and say so in the notice.
fn export_dir() -> std::path::PathBuf {
    match std::env::var_os("HERMES_DESKTOP_EXPORT_DIR") {
        Some(p) => std::path::PathBuf::from(p),
        None => data_dir().join("exports"),
    }
}

fn import_dir() -> std::path::PathBuf {
    match std::env::var_os("HERMES_DESKTOP_EXPORT_DIR") {
        Some(p) => std::path::PathBuf::from(p).join("imports"),
        None => data_dir().join("imports"),
    }
}

fn data_dir() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/share"));
    base.join("hermes-desktop-rs")
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

/// Choices an `approval` server request offers. Mirrors `tui_gateway/server.py::
/// _approval_request_payload`: an explicit `choices` list wins; otherwise `once`, then
/// `session` unless smart-denied or `allow_session == false`, then `always` unless
/// `allow_permanent == false`, then `deny`.
pub fn approval_choices(params: &Value) -> Vec<String> {
    if let Some(list) = params.get("choices").and_then(Value::as_array) {
        return list.iter().filter_map(Value::as_str).map(str::to_owned).collect();
    }
    let flag = |k: &str| params.get(k).and_then(Value::as_bool);
    let mut out = vec!["once".to_owned()];
    if flag("smart_denied") != Some(true) && flag("allow_session") != Some(false) {
        out.push("session".into());
        if flag("allow_permanent") != Some(false) {
            out.push("always".into());
        }
    }
    out.push("deny".into());
    out
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

    #[test]
    fn approval_choice_rules() {
        let c = |v: Value| approval_choices(&v);
        assert_eq!(c(json!({})), ["once", "session", "always", "deny"]);
        assert_eq!(c(json!({"choices": ["once", "deny"]})), ["once", "deny"]);
        assert_eq!(c(json!({"allow_permanent": false})), ["once", "session", "deny"]);
        assert_eq!(c(json!({"allow_session": false})), ["once", "deny"]);
        assert_eq!(c(json!({"smart_denied": true})), ["once", "deny"]);
    }
}
