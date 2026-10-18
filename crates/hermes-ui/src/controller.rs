//! Async controller: owns backend + client + transcript; talks to the UI only through
//! `invoke_from_event_loop` (UI ← core) and an unbounded command channel (UI → core).

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use hermes_core::attachments::{self, AttachedImage, ClipboardError};
use hermes_core::backend::{self, ManagedBackend};
use hermes_core::highlight::{self, Highlighter, Item};
use hermes_core::http::{Rest, profile_query};
use hermes_core::models::{self, CatalogProvider};
use hermes_core::prefs::Prefs;
use hermes_core::projects::{self, Project, ProjectList};
use hermes_core::protocol::{
    ModelOptionsResult, SessionCreateResult, SessionListResult, SessionListRow, SessionResumeResult,
};
use hermes_core::sessions::{cycle_index, display_title, sidebar_groups};
use hermes_core::settings::{self, Store};
use hermes_core::transcript::{Change, Transcript};
use hermes_core::{Client, ClientEvent};
use serde_json::{Value, json};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel, Weak};
use tokio::sync::mpsc;

mod composer;
mod inputs;
mod sidebar;
mod slash;

pub use composer::ComposerCmd;
pub use inputs::{InputCmd, Secret};

use crate::browser::BrowserCmd;
use crate::capabilities::CapCmd;
use crate::rows::{Ctx as RowCtx, hl_job_id, hl_jobs, hl_result_id, seg_block, to_row};
use crate::widgets::{self, Ctx, Outcome};
use crate::{
    AppState, AppWindow, ApprovalInfo, CapState, ChatRow, EditModelRow, ModelItem, NavEntry, ProjectItem, ProjectState,
    ScopeChip, SearchHit, SessionItem, SettingRowUI, SettingsState, Theme,
};

/// Chats the sidebar lists (`session.list`); pinned flags are read for the same window.
pub(crate) const SESSION_LIST_LIMIT: u32 = 200;

/// Commands from the UI thread to the controller.
#[derive(Debug, Clone)]
pub enum Cmd {
    Send(String),
    Queue(String),
    SaveDraft {
        session: String,
        text: String,
    },
    SubmissionFailed {
        session_id: String,
        text: String,
        message: String,
    },
    Stop,
    NewChat,
    Open(String),
    Delete(String),
    PinSession(String, bool),
    RenameSession(String, String),
    ArchiveSession(String),
    Search(String),
    /// Backend message-search hits for search generation `.0` (sidebar.rs).
    SearchResults(u64, Result<Vec<hermes_core::api::sessions::SessionSearchHit>, String>),
    /// Open the chat this many places away in sidebar order (Ctrl+Tab = 1, Ctrl+Shift+Tab = -1).
    Cycle(i32),
    RefreshModels,
    /// "Refresh models" in the picker: re-probe providers (`model.options` `refresh`).
    ReloadModels,
    ModelQuery(String),
    ModelEditQuery(String),
    ToggleModel {
        provider: String,
        model: String,
    },
    ToggleProviderVisible(String),
    ToggleProviderCollapse(String),
    FavoriteModel {
        provider: String,
        model: String,
    },
    AddCustomModel {
        provider: String,
        model: String,
    },
    RemoveCustomModel {
        provider: String,
        model: String,
    },
    ResetModelVisibility,
    SelectEffort(String),
    RefreshProjects,
    SelectProject(String),
    SaveProject {
        id: String,
        name: String,
        path: String,
    },
    SelectModel {
        provider: String,
        model: String,
    },
    Approve {
        request_id: String,
        choice: String,
    },
    Settings(SettingsCmd),
    /// Ctrl+V in the composer: attach the clipboard image, if there is one.
    PasteImage,
    /// Remove the queued attachment at this index.
    RemoveAttachment(i32),
    /// Capabilities-screen intents, forwarded to its own task (`capabilities.rs`).
    Caps(CapCmd),
    /// Agent question card (clarify / sudo / secret).
    Input(InputCmd),
    /// Composer history, edit-and-resend and file attach (`controller/composer.rs`).
    Composer(ComposerCmd),
    /// Show a banner from a background task.
    Banner(String, bool),
    /// The theme flipped: code colours are baked into rows, so rebuild them.
    Retheme,
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
    st.on_save_draft(move |session, text| {
        let _ = t.send(Cmd::SaveDraft {
            session: session.into(),
            text: text.into(),
        });
    });
    let t = tx.clone();
    st.on_send(move |s| {
        let s = s.trim().to_owned();
        if !s.is_empty() {
            let _ = t.send(Cmd::Send(s));
        }
    });
    let t = tx.clone();
    st.on_queue(move |text| {
        let _ = t.send(Cmd::Queue(text.into()));
    });
    let t = tx.clone();
    st.on_inspect_queue(move || {
        let _ = t.send(Cmd::Send("/queue list".into()));
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
    st.on_reload_models(move || {
        let _ = t.send(Cmd::ReloadModels);
    });
    let t = tx.clone();
    st.on_select_model(move |p, m| {
        let _ = t.send(Cmd::SelectModel {
            provider: p.into(),
            model: m.into(),
        });
    });
    // Picker curation. Each is one stored device preference (visibility, favourites,
    // custom ids, collapsed groups) plus a re-render; the backend is not involved.
    let t = tx.clone();
    st.on_model_query_changed(move |q| {
        let _ = t.send(Cmd::ModelQuery(q.into()));
    });
    let t = tx.clone();
    st.on_model_edit_query_changed(move |q| {
        let _ = t.send(Cmd::ModelEditQuery(q.into()));
    });
    let t = tx.clone();
    st.on_toggle_model(move |p, m| {
        let _ = t.send(Cmd::ToggleModel {
            provider: p.into(),
            model: m.into(),
        });
    });
    let t = tx.clone();
    st.on_toggle_provider_visible(move |p| {
        let _ = t.send(Cmd::ToggleProviderVisible(p.into()));
    });
    let t = tx.clone();
    st.on_toggle_provider_collapse(move |p| {
        let _ = t.send(Cmd::ToggleProviderCollapse(p.into()));
    });
    let t = tx.clone();
    st.on_favorite_model(move |p, m| {
        let _ = t.send(Cmd::FavoriteModel {
            provider: p.into(),
            model: m.into(),
        });
    });
    let t = tx.clone();
    st.on_add_custom_model(move |p, m| {
        let _ = t.send(Cmd::AddCustomModel {
            provider: p.into(),
            model: m.into(),
        });
    });
    let t = tx.clone();
    st.on_remove_custom_model(move |p, m| {
        let _ = t.send(Cmd::RemoveCustomModel {
            provider: p.into(),
            model: m.into(),
        });
    });
    let t = tx.clone();
    st.on_reset_model_visibility(move || {
        let _ = t.send(Cmd::ResetModelVisibility);
    });
    let t = tx.clone();
    st.on_select_effort(move |level| {
        let _ = t.send(Cmd::SelectEffort(level.into()));
    });
    let t = tx.clone();
    ui.global::<ProjectState>().on_refresh(move || {
        let _ = t.send(Cmd::RefreshProjects);
    });
    let t = tx.clone();
    ui.global::<ProjectState>().on_select(move |id| {
        let _ = t.send(Cmd::SelectProject(id.into()));
    });
    let t = tx.clone();
    ui.global::<ProjectState>().on_save(move |id, name, path| {
        let _ = t.send(Cmd::SaveProject {
            id: id.into(),
            name: name.into(),
            path: path.into(),
        });
    });
    let t = tx.clone();
    st.on_paste_image(move || {
        let _ = t.send(Cmd::PasteImage);
    });
    let t = tx.clone();
    st.on_remove_attachment(move |i| {
        let _ = t.send(Cmd::RemoveAttachment(i));
    });
    let t = tx.clone();
    st.on_approve(move |id, choice| {
        let _ = t.send(Cmd::Approve {
            request_id: id.into(),
            choice: choice.into(),
        });
    });
    inputs::wire(ui, &tx);
    composer::wire(ui, &tx);
    let t = tx.clone();
    st.on_retheme(move || {
        let _ = t.send(Cmd::Retheme);
    });
    sidebar::wire(ui, &tx);
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

    let c = ui.global::<CapState>();
    let t = tx.clone();
    c.on_refresh(move || {
        let _ = t.send(Cmd::Caps(CapCmd::Refresh));
    });
    let t = tx.clone();
    c.on_set_tab(move |i| {
        let _ = t.send(Cmd::Caps(CapCmd::Tab(i)));
    });
    let t = tx.clone();
    c.on_set_scope(move |name| {
        let _ = t.send(Cmd::Caps(CapCmd::Scope(name.into())));
    });
    let t = tx.clone();
    c.on_filter(move |q| {
        let _ = t.send(Cmd::Caps(CapCmd::Filter(q.into())));
    });
    let t = tx.clone();
    c.on_toggle(move |id, on| {
        let _ = t.send(Cmd::Caps(CapCmd::Toggle { id: id.into(), on }));
    });
    let t = tx.clone();
    c.on_act(move |id, action| {
        let _ = t.send(Cmd::Caps(CapCmd::Act {
            id: id.into(),
            action: action.into(),
        }));
    });
    // Form and editor text are read on the UI thread when the button is clicked.
    let (t, w) = (tx.clone(), ui.as_weak());
    c.on_create(move || {
        if let Some(ui) = w.upgrade() {
            let s = ui.global::<CapState>();
            let fields = [s.get_f1().into(), s.get_f2().into(), s.get_f3().into()];
            let _ = t.send(Cmd::Caps(CapCmd::Create(fields)));
        }
    });
    let (t, w) = (tx.clone(), ui.as_weak());
    c.on_save_detail(move || {
        if let Some(ui) = w.upgrade() {
            let text = ui.global::<CapState>().get_detail_text().into();
            let _ = t.send(Cmd::Caps(CapCmd::SaveDetail(text)));
        }
    });

    ui.global::<AppState>()
        .set_greeting(greeting_for_hour(local_hour()).into());
    ui.global::<AppState>().set_user_name(user_name().into());

    let weak = ui.as_weak();
    rt.spawn(async move {
        Controller::run(weak, rx, tx).await;
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
    // Widget panels own the rest of the visual prefs (the palette and cached fonts); they
    // are applied here so an edit and a fresh start look the same.
    crate::widgets::apply_pref(ui, key, value);
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
pub(crate) fn ui(weak: &Weak<AppWindow>, f: impl FnOnce(&AppWindow) + Send + 'static) {
    let _ = weak.upgrade_in_event_loop(move |w| f(&w));
}

struct Controller {
    weak: Weak<AppWindow>,
    commands: mpsc::UnboundedSender<Cmd>,
    client: Client,
    _backend: Arc<ManagedBackend>,
    transcript: Transcript,
    queues: hermes_core::commands::PromptQueues,
    queue_paused: std::collections::HashSet<String>,
    side_results: std::collections::HashMap<String, Vec<String>>,
    runtime_stores: std::collections::HashMap<String, String>,
    completion_task: Option<tokio::task::JoinHandle<()>>,
    /// Runtime session id of the open chat (None = new-chat screen).
    session: Option<String>,
    /// Stored id of the open chat (sidebar highlight / delete).
    stored: Option<String>,
    all_sessions: Vec<SessionListRow>,
    /// Stored ids the backend marks pinned (shown first under "Pinned").
    pinned: std::collections::HashSet<String>,
    /// Message-content matches for `query` that the title filter did not already show.
    search_hits: Vec<SessionListRow>,
    /// Stored ids in the order the sidebar shows them (after search + grouping).
    visible_ids: Vec<String>,
    query: String,
    /// Bumped per sidebar query; background searches compare it before and after.
    search_gen: std::sync::Arc<std::sync::atomic::AtomicU64>,
    drafts: std::collections::HashMap<String, String>,
    dirty: Vec<usize>,
    reset: bool,
    pending_approval: Option<String>,
    /// Open clarify / sudo / secret requests, oldest first (parked per chat).
    inputs: Vec<hermes_core::input_requests::PendingInput>,
    /// Model picked on the new-chat screen before a session exists: (provider, model).
    /// Sent with `session.create` (as the Electron app does); `config.set model` needs a
    /// live session and answers 4001 without one.
    draft_model: Option<(String, String)>,
    draft_effort: Option<String>,
    /// Last `model.options` catalog (backend order), before custom ids are merged.
    catalog: Vec<CatalogProvider>,
    /// Per-provider note for the rows ("not signed in"): slug → hint.
    provider_hints: std::collections::HashMap<String, String>,
    /// The backend's (or draft's) current pick — the tick in the picker.
    catalog_provider: String,
    catalog_model: String,
    /// Picker and Edit-models search boxes.
    model_query: String,
    model_edit_query: String,
    projects: Vec<Project>,
    project: Option<Project>,
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
    /// The Capabilities screen's own task.
    caps: mpsc::UnboundedSender<CapCmd>,
    /// Images queued on the backend for the next turn of `session`.
    attachments: Vec<AttachedImage>,
    /// Composer history/edit state and files staged for the next turn.
    composer: composer::ComposerState,
    /// The in-app browser's own task (`crate::browser`).
    browser: mpsc::UnboundedSender<BrowserCmd>,
    /// Code highlighter, started on the first code block (never before first frame).
    hl: Option<Highlighter>,
    hl_tx: mpsc::UnboundedSender<highlight::Done>,
    /// Arrived highlight results by segment id (rows.rs `seg_id`).
    hl_items: HashMap<u64, Arc<Item>>,
    /// Content key last queued per segment id, so an unchanged block is not re-sent.
    hl_sent: HashMap<u64, u64>,
    /// Bumped on every reset; tags highlight jobs so late results can be recognised.
    hl_epoch: u16,
}

impl Controller {
    async fn run(weak: Weak<AppWindow>, mut cmds: mpsc::UnboundedReceiver<Cmd>, commands: mpsc::UnboundedSender<Cmd>) {
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
        let caps = {
            let e = &backend.endpoint;
            crate::capabilities::spawn(Rest::new(e.host.clone(), e.port, e.token.clone()), weak.clone())
        };
        let browser = crate::browser::spawn(client.clone(), weak.clone());
        let (hl_tx, mut hl_rx) = mpsc::unbounded_channel::<highlight::Done>();
        let mut c = Controller {
            caps,
            browser,
            attachments: Vec::new(),
            composer: composer::ComposerState::default(),
            weak: weak.clone(),
            commands,
            client,
            _backend: backend,
            transcript: Transcript::new(),
            queues: Default::default(),
            queue_paused: Default::default(),
            side_results: Default::default(),
            runtime_stores: Default::default(),
            completion_task: None,
            session: None,
            stored: None,
            draft_model: None,
            draft_effort: None,
            catalog: Vec::new(),
            provider_hints: std::collections::HashMap::new(),
            catalog_provider: String::new(),
            catalog_model: String::new(),
            model_query: String::new(),
            model_edit_query: String::new(),
            projects: vec![],
            project: None,
            all_sessions: vec![],
            pinned: Default::default(),
            search_hits: vec![],
            visible_ids: vec![],
            query: String::new(),
            search_gen: Default::default(),
            drafts: std::collections::HashMap::new(),
            dirty: vec![],
            reset: false,
            pending_approval: None,
            inputs: Vec::new(),
            rest: None,
            profiles: Vec::new(),
            scope: String::new(),
            config: Value::Null,
            schema: None,
            prefs: Prefs::load(hermes_core::prefs::default_path()),
            settings_page: ("appearance".into(), "general".into()),
            settings_hit: String::new(),
            reset_armed: false,
            hl: None,
            hl_tx,
            hl_items: HashMap::new(),
            hl_sent: HashMap::new(),
            hl_epoch: 0,
        };
        ui(&weak, |w| {
            let st = w.global::<AppState>();
            st.set_connection("open".into());
            st.set_rows(ModelRc::new(VecModel::<ChatRow>::default()));
        });
        let saved_prefs: Vec<(String, String)> = c
            .prefs
            .all()
            .keys()
            .map(|k| (k.clone(), c.prefs.get_str(k, "")))
            .collect();
        ui(&weak, move |w| {
            for (key, value) in saved_prefs {
                apply_device_pref(w, &key, &value);
            }
        });
        c.refresh_projects(true).await;
        c.refresh_sessions().await;
        crate::perf::mark("controller: session list fetched");
        c.refresh_model_label().await;
        c.refresh_effort().await;
        crate::perf::mark("controller: model label fetched");
        c.load_about().await;
        if let Some(rest) = c.rest()
            && let Ok(config) = rest.get("/api/config").await
        {
            let font = settings::get(&config, "desktop.font_family")
                .and_then(Value::as_str)
                .unwrap_or("Inter")
                .to_owned();
            let mono = settings::get(&config, "terminal.font_family")
                .and_then(Value::as_str)
                .unwrap_or("JetBrains Mono")
                .to_owned();
            ui(&weak, move |w| {
                w.global::<Theme>()
                    .set_ui_font(if font.is_empty() { "Inter".into() } else { font.into() });
                w.global::<Theme>().set_mono_font(if mono.is_empty() {
                    "JetBrains Mono".into()
                } else {
                    mono.into()
                });
            });
        }
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
                Some((job, item)) = hl_rx.recv() => {
                    // Results arrive off-thread; repaint the owning row on the next frame.
                    // A result from before the last reset belongs to another transcript.
                    if let Some(id) = hl_result_id(job, c.hl_epoch) {
                        c.hl_items.insert(id, item);
                        c.dirty.push(seg_block(id));
                    }
                }
                _ = tick.tick(), if c.reset || !c.dirty.is_empty() => c.flush(),
            }
        }
    }

    /// Push changed rows to the UI (coalesced to at most one patch per frame).
    fn flush(&mut self) {
        if self.reset {
            self.reset = false;
            self.dirty.clear();
            // A reset is a new transcript (or theme): segment ids refer to other blocks now.
            self.hl_items.clear();
            self.hl_sent.clear();
            self.hl_epoch = self.hl_epoch.wrapping_add(1);
            for i in 0..self.transcript.blocks.len() {
                self.queue_highlight(i);
            }
            // Blocks are Send; Slint rows (which hold Rc models) are built on the UI thread.
            let blocks = self.transcript.blocks.clone();
            let hl = self.hl_items.clone();
            let (running, status) = (self.transcript.running, self.transcript.status.clone());
            ui(&self.weak, move |w| {
                let st = w.global::<AppState>();
                let ctx = RowCtx {
                    dark: w.global::<Theme>().get_dark(),
                    hl: &hl,
                };
                let rows: Vec<ChatRow> = blocks.iter().enumerate().map(|(i, b)| to_row(i, b, &ctx)).collect();
                st.set_rows(ModelRc::new(VecModel::from(rows)));
                st.set_running(running);
                st.set_status(status.into());
            });
            return;
        }
        let mut idx = std::mem::take(&mut self.dirty);
        idx.sort_unstable();
        idx.dedup();
        for &i in &idx {
            self.queue_highlight(i);
        }
        let patch: Vec<(usize, hermes_core::transcript::Block)> = idx
            .into_iter()
            .filter_map(|i| self.transcript.blocks.get(i).map(|b| (i, b.clone())))
            .collect();
        let hl: HashMap<u64, Arc<Item>> = self
            .hl_items
            .iter()
            .filter(|(id, _)| patch.iter().any(|(i, _)| seg_block(**id) == *i))
            .map(|(id, it)| (*id, Arc::clone(it)))
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
            let ctx = RowCtx {
                dark: w.global::<Theme>().get_dark(),
                hl: &hl,
            };
            for (i, b) in patch {
                let r = to_row(i, &b, &ctx);
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

    /// Queue highlight jobs for block `i`'s code fences whose content changed. Never
    /// blocks: the worker parses on its own thread and answers through `hl_tx`.
    fn queue_highlight(&mut self, i: usize) {
        let Some(b) = self.transcript.blocks.get(i) else {
            return;
        };
        for (id, text, lang) in hl_jobs(i, b) {
            let key = highlight::block_key(&lang, &text);
            if self.hl_sent.insert(id, key) == Some(key) {
                continue;
            }
            let hl = self.hl.get_or_insert_with(|| {
                let tx = self.hl_tx.clone();
                Highlighter::launch(move |done| {
                    let _ = tx.send(done);
                })
            });
            hl.put(hl_job_id(id, self.hl_epoch), text, lang);
        }
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
            Cmd::Queue(text) => {
                if !self.queue_command(&format!("add {text}")).await {
                    self.restore_rejected_queue(&text);
                }
            }
            Cmd::SaveDraft { session, text } => {
                self.composer_text_changed(&text);
                self.drafts.insert(session, text.clone());
                self.completions(text);
            }
            Cmd::SubmissionFailed {
                session_id,
                text,
                message,
            } => {
                let key = self
                    .runtime_stores
                    .get(&session_id)
                    .cloned()
                    .unwrap_or_else(|| session_id.clone());
                self.queue_paused.insert(key.clone());
                let draft = self.drafts.entry(key.clone()).or_default();
                if draft.is_empty() {
                    *draft = text.clone();
                }
                let notice = format!("Message failed: {message}\nUnsent: {text}");
                if self.queue_key() == Some(key.as_str()) {
                    // An old runtime failure must not stop a newer resumed turn.
                    if self.session.as_deref() == Some(&session_id) {
                        self.transcript.running = false;
                        self.transcript.status.clear();
                    }
                    self.notice(notice, true);
                    self.banner(format!("Message failed: {message}"), true);
                    ui(&self.weak, move |w| {
                        let st = w.global::<AppState>();
                        if st.get_active_session().as_str() == key && st.get_draft().is_empty() {
                            st.set_draft(text.into());
                        }
                    });
                } else {
                    self.side_results.entry(key).or_default().push(notice);
                    self.banner(
                        "A message failed in another chat. Reopen it to recover the unsent prompt.".into(),
                        true,
                    );
                }
            }
            Cmd::Caps(cmd) => {
                let _ = self.caps.send(cmd);
            }
            Cmd::Input(cmd) => self.input_cmd(cmd),
            Cmd::Composer(cmd) => self.composer_cmd(cmd).await,
            Cmd::Banner(text, error) => self.banner(text, error),
            Cmd::PasteImage => self.paste_image().await,
            Cmd::RemoveAttachment(i) => self.remove_attachment(i).await,
            Cmd::Retheme => self.reset = true,
            Cmd::Stop => {
                if let Some(key) = self.queue_key().map(str::to_owned) {
                    self.queue_paused.insert(key);
                }
                if let Some(sid) = self.session.clone()
                    && let Err(e) = self
                        .client
                        .request("session.interrupt", json!({"session_id": sid}))
                        .await
                {
                    self.banner(format!("Could not stop chat: {e}"), true);
                }
            }
            Cmd::NewChat => {
                self.clear_attachments();
                self.draft_model = None;
                self.draft_effort = None;
                self.session = None;
                self.stored = None;
                self.publish_queue();
                self.transcript = Transcript::new();
                self.reset = true;
                self.flush();
                ui(&self.weak, |w| {
                    let st = w.global::<AppState>();
                    st.set_active_session(SharedString::new());
                    st.set_runtime_session(SharedString::new());
                    st.set_completions(ModelRc::default());
                    st.set_session_title("New chat".into());
                    st.set_draft(SharedString::new());
                    st.set_approval(ApprovalInfo::default());
                });
                self.publish_input();
                self.publish_project();
                self.refresh_model_label().await;
                self.refresh_effort().await;
            }
            Cmd::Open(id) => self.open(id).await,
            Cmd::Delete(id) => self.delete_session(id).await,
            Cmd::PinSession(id, on) => self.pin_session(id, on).await,
            Cmd::RenameSession(id, title) => self.rename_session(id, title).await,
            Cmd::ArchiveSession(id) => self.archive_session(id).await,
            Cmd::Search(q) => self.search_sessions(q),
            Cmd::SearchResults(generation, hits) => self.search_results(generation, hits),
            Cmd::Cycle(step) => {
                if let Some(i) = cycle_index(&self.visible_ids, self.stored.as_deref(), step) {
                    let id = self.visible_ids[i].clone();
                    self.open(id).await;
                }
            }
            Cmd::RefreshModels => self.refresh_models(false).await,
            Cmd::ReloadModels => self.refresh_models(true).await,
            Cmd::ModelQuery(q) => {
                self.model_query = q;
                self.publish_models();
            }
            Cmd::ModelEditQuery(q) => {
                self.model_edit_query = q;
                self.publish_models();
            }
            Cmd::ToggleModel { provider, model } => self.toggle_model(&provider, &model),
            Cmd::ToggleProviderVisible(provider) => self.set_provider_visible(&provider),
            Cmd::ToggleProviderCollapse(provider) => self.toggle_provider_collapse(&provider),
            Cmd::FavoriteModel { provider, model } => self.toggle_favorite(&provider, &model),
            Cmd::AddCustomModel { provider, model } => self.add_custom(&provider, &model),
            Cmd::RemoveCustomModel { provider, model } => self.remove_custom(&provider, &model),
            Cmd::ResetModelVisibility => self.reset_model_visibility(),
            Cmd::SelectEffort(level) => self.select_effort(level).await,
            Cmd::RefreshProjects => self.refresh_projects(false).await,
            Cmd::SelectProject(id) => self.select_project(id).await,
            Cmd::SaveProject { id, name, path } => self.save_project(id, name, path).await,
            Cmd::SelectModel { provider, model } => {
                let Some(sid) = self.session.clone() else {
                    // New-chat screen: remember the pick; it rides on session.create.
                    let label = short_model(&model);
                    self.draft_model = Some((provider, model));
                    self.publish_models();
                    ui(&self.weak, move |w| {
                        w.global::<AppState>().set_model_label(label.into())
                    });
                    return;
                };
                let params = json!({
                    "key": "model",
                    "value": format!("{model} --provider {provider} --session"),
                    "session_id": sid,
                });
                match self.client.request("config.set", params).await {
                    Ok(result) => {
                        if result.get("confirm_required").and_then(Value::as_bool) == Some(true) {
                            self.banner(
                                result
                                    .get("confirm_message")
                                    .and_then(Value::as_str)
                                    .unwrap_or("This model requires confirmation in Hermes before switching.")
                                    .to_owned(),
                                true,
                            );
                        } else if result.get("deferred").and_then(Value::as_bool) == Some(true) {
                            self.banner("Model change queued for the next turn.".into(), false);
                        } else {
                            // Move the picker's tick at once; the label re-read confirms it.
                            self.catalog_provider = provider.clone();
                            self.catalog_model = model.clone();
                            self.publish_models();
                            self.refresh_model_label().await;
                            self.refresh_effort().await;
                        }
                    }
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

    // ── Project scope and per-chat reasoning ──────────────────────────────────

    async fn refresh_projects(&mut self, restore_active: bool) {
        ui(&self.weak, |w| w.global::<ProjectState>().set_loading(true));
        match self.client.call::<ProjectList>("projects.list", json!({})).await {
            Ok(list) => {
                self.projects = list.projects.into_iter().filter(|p| !p.archived).collect();
                if restore_active {
                    self.project = list
                        .active_id
                        .and_then(|id| self.projects.iter().find(|p| p.id == id).cloned());
                } else if let Some(id) = self.project.as_ref().map(|p| p.id.clone()) {
                    self.project = self.projects.iter().find(|p| p.id == id).cloned();
                }
                let items: Vec<ProjectItem> = self
                    .projects
                    .iter()
                    .map(|p| ProjectItem {
                        id: p.id.as_str().into(),
                        name: p.name.as_str().into(),
                        path: p.root().into(),
                    })
                    .collect();
                ui(&self.weak, move |w| {
                    let st = w.global::<ProjectState>();
                    st.set_items(Rc::new(VecModel::from(items)).into());
                    st.set_error(SharedString::new());
                });
                self.publish_project();
            }
            Err(e) => self.project_error(format!("Could not load projects: {e}")),
        }
        ui(&self.weak, |w| w.global::<ProjectState>().set_loading(false));
    }

    fn publish_project(&self) {
        let id = self.project.as_ref().map(|p| p.id.clone()).unwrap_or_default();
        let name = self.project.as_ref().map(|p| p.name.clone()).unwrap_or_default();
        let root = self.project.as_ref().map(|p| short_path(p.root())).unwrap_or_default();
        let draft = self.session.is_none();
        ui(&self.weak, move |w| {
            let st = w.global::<ProjectState>();
            st.set_active_id(id.into());
            st.set_active_name(name.into());
            if draft {
                w.global::<AppState>().set_cwd_label(root.into());
            }
        });
    }

    fn project_error(&self, message: String) {
        let error = message.clone();
        ui(&self.weak, move |w| w.global::<ProjectState>().set_error(error.into()));
        self.banner(message, true);
    }

    async fn select_project(&mut self, id: String) {
        let project = if id.is_empty() {
            None
        } else {
            let Some(project) = self.projects.iter().find(|p| p.id == id).cloned() else {
                return self.project_error("Project no longer exists. Refresh the project list.".into());
            };
            if let Err(error) = projects::existing_folder(
                project.root(),
                &std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()),
            ) {
                return self.project_error(error);
            }
            Some(project)
        };
        if let Err(e) = self
            .client
            .request(
                "projects.set_active",
                json!({"id": if id.is_empty() { Value::Null } else { json!(id) }}),
            )
            .await
        {
            return self.project_error(format!("Could not select project: {e}"));
        }
        // Read back the pointer before changing the local scope.
        match self.client.call::<ProjectList>("projects.list", json!({})).await {
            Ok(list) if list.active_id.as_deref().unwrap_or_default() == id => {}
            Ok(_) => return self.project_error("Project selection was not saved. Try again.".into()),
            Err(e) => return self.project_error(format!("Could not verify project selection: {e}")),
        }
        self.project = project;
        self.banner(String::new(), false);
        Box::pin(self.handle_cmd(Cmd::NewChat)).await;
        ui(&self.weak, |w| w.global::<ProjectState>().set_open(false));
    }

    async fn save_project(&mut self, id: String, name: String, path: String) {
        let name = name.trim();
        if name.is_empty() {
            return self.project_error("Enter a project name.".into());
        }
        let (method, params) = if id.is_empty() {
            let path = match projects::existing_folder(
                &path,
                &std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()),
            ) {
                Ok(p) => p,
                Err(e) => return self.project_error(e),
            };
            (
                "projects.create",
                json!({"name": name, "folders": [path], "primary_path": path, "use": false}),
            )
        } else {
            ("projects.update", json!({"id": id, "name": name}))
        };
        match self.client.request(method, params).await {
            Ok(result) => {
                let saved_id = result
                    .pointer("/project/id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                self.refresh_projects(false).await;
                if !self.projects.iter().any(|p| p.id == saved_id && p.name == name) {
                    return self
                        .project_error("Project save could not be verified. Refresh before trying again.".into());
                }
                self.banner(String::new(), false);
                ui(&self.weak, |w| {
                    let st = w.global::<ProjectState>();
                    st.set_edit_id(SharedString::new());
                    st.set_name(SharedString::new());
                    st.set_path(SharedString::new());
                });
            }
            Err(e) => self.project_error(format!("Could not save project: {e}")),
        }
    }

    async fn refresh_effort(&self) {
        if self.session.is_none()
            && let Some(level) = &self.draft_effort
        {
            let level = level.clone();
            ui(&self.weak, move |w| {
                w.global::<AppState>().set_effort_label(level.into())
            });
            return;
        }
        let mut params = json!({"key": "reasoning"});
        if let Some(sid) = &self.session {
            params["session_id"] = json!(sid);
        }
        match self.client.request("config.get", params).await {
            Ok(result) => {
                let level = result
                    .get("value")
                    .and_then(Value::as_str)
                    .unwrap_or("medium")
                    .to_owned();
                ui(&self.weak, move |w| {
                    w.global::<AppState>().set_effort_label(level.into())
                });
            }
            Err(e) => self.banner(format!("Could not load reasoning: {e}"), true),
        }
    }

    async fn select_effort(&mut self, level: String) {
        if !projects::REASONING_LEVELS.contains(&level.as_str()) {
            return self.banner("Unknown reasoning level.".into(), true);
        }
        let Some(sid) = &self.session else {
            self.draft_effort = Some(level);
            self.refresh_effort().await;
            return;
        };
        match self
            .client
            .request(
                "config.set",
                json!({"key": "reasoning", "value": level, "scope": "session", "session_id": sid}),
            )
            .await
        {
            Ok(_) => self.refresh_effort().await,
            Err(e) => self.banner(format!("Could not change reasoning: {e}"), true),
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
        let t0 = std::time::Instant::now();
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
        self.load_widgets().await;
        self.set_loading(false);
        // Open time = keypress → the page's rows handed to the UI (docs/PERF.md).
        tracing::info!(
            target: "settings",
            elapsed_ms = format_args!("{:.1}", t0.elapsed().as_secs_f64() * 1000.0),
            "settings panel loaded"
        );
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
                self.load_widgets().await;
            }
            SettingsCmd::Search(q) => self.publish_hits(&q),
            SettingsCmd::Jump { group, page, setting } => {
                let (g, p) = resolve_page(&group, &page);
                self.settings_page = (g, p);
                self.settings_hit = setting;
                self.publish_settings();
                self.load_widgets().await;
            }
            SettingsCmd::Scope(name) => {
                if name != self.scope {
                    self.scope = name;
                    self.load_scoped_config().await;
                    self.publish_settings();
                    self.load_widgets().await;
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
                        self.config = match rest.get(&path).await {
                            Ok(config) if settings::get(&config, key) == Some(&parsed) => config,
                            Ok(_) => return self.settings_error(format!("The backend did not save {key}.")),
                            Err(e) => return self.settings_error(format!("Could not verify {key}: {e}")),
                        };
                        if self.scope.is_empty() && matches!(key, "desktop.font_family" | "terminal.font_family") {
                            let key = key.to_owned();
                            let value = parsed.as_str().unwrap_or_default().to_owned();
                            ui(&self.weak, move |w| {
                                if key == "desktop.font_family" {
                                    w.global::<Theme>().set_ui_font(if value.is_empty() {
                                        "Inter".into()
                                    } else {
                                        value.into()
                                    });
                                } else {
                                    w.global::<Theme>().set_mono_font(if value.is_empty() {
                                        "JetBrains Mono".into()
                                    } else {
                                        value.into()
                                    });
                                }
                            });
                        }
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
        self.load_widgets().await;
    }

    /// Everything a widget module needs (see [`crate::widgets`]).
    fn widget_ctx(&mut self) -> Option<Ctx> {
        let rest = self.rest()?;
        Some(Ctx {
            rest,
            weak: self.weak.clone(),
            scope: self.scope.clone(),
        })
    }

    /// Load the widget panels of the open page. Runs after every publish, so a widget
    /// always reflects the same profile and config state as the rows around it.
    async fn load_widgets(&mut self) {
        let (group, page) = self.settings_page.clone();
        let ids: Vec<&'static str> = settings::settings_for(&group, &page)
            .filter(|s| widgets::owns(s.id))
            .map(|s| s.id)
            .collect();
        if ids.is_empty() {
            return;
        }
        let Some(ctx) = self.widget_ctx() else { return };
        for id in ids {
            widgets::load(id, &ctx).await;
        }
    }

    /// Apply what a widget action asked the panel to do.
    async fn apply_widget_outcome(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::None => {}
            Outcome::Notice(text) => self.settings_notice(text, false),
            Outcome::Error(text) => self.settings_error(text),
            Outcome::Edit(id, value) => self.edit_setting(&id, &value).await,
            Outcome::Reload => {
                self.load_scoped_config().await;
                self.publish_settings();
                self.load_widgets().await;
            }
        }
    }

    /// Row actions: widget panels first (`<area>:<verb>[:<arg>]`, see [`crate::widgets`]),
    /// then the singleton registry actions.
    async fn settings_action(&mut self, id: &str) {
        if let Some(area) = id.split(':').next()
            && widgets::AREAS.contains(&area)
        {
            let Some(ctx) = self.widget_ctx() else { return };
            let outcome = widgets::action(id, &ctx).await;
            return self.apply_widget_outcome(outcome).await;
        }
        match id {
            // The original raises a *native* notification here; this client has no OS
            // notification path yet, and the gateway API exposes no notify route (the old
            // code posted to /api/desktop/notify, which answers 405) — so say that plainly
            // instead of claiming a delivery.
            "notifications.test" | "test-notification" => self.settings_error(
                "This client cannot raise desktop notifications yet — the switches below are stored, but nothing notifies."
                    .to_owned(),
            ),
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
                if let Some((provider, model)) = &self.draft_model {
                    p["model"] = json!(model);
                    p["provider"] = json!(provider);
                }
                if let Some(effort) = &self.draft_effort {
                    p["reasoning_effort"] = json!(effort);
                }
                if let Some(project) = &self.project {
                    p["cwd"] = json!(project.root());
                    p["cwd_explicit"] = json!(true);
                }
                p
            })
            .await
        {
            Ok(r) => {
                self.session = Some(r.session_id.clone());
                self.stored = Some(r.stored_session_id.clone()).filter(|s| !s.is_empty());
                if let Some(stored) = &self.stored {
                    self.runtime_stores.insert(r.session_id.clone(), stored.clone());
                }
                let stored = self.stored.clone().unwrap_or_default();
                let runtime = r.session_id.clone();
                self.draft_model = None;
                self.draft_effort = None;
                let info = r.info;
                ui(&self.weak, move |w| {
                    let st = w.global::<AppState>();
                    st.set_active_session(stored.into());
                    st.set_runtime_session(runtime.into());
                    st.set_model_label(short_model(&info.model).into());
                    st.set_effort_label(info.reasoning_effort.into());
                    st.set_cwd_label(short_path(&info.cwd).into());
                });
                self.refresh_effort().await;
                Some(r.session_id)
            }
            Err(e) => {
                self.banner(format!("Could not start a chat: {e}"), true);
                None
            }
        }
    }

    async fn send(&mut self, text: String) {
        if text.starts_with('/') {
            self.slash(text).await;
            return;
        }
        if self.transcript.running {
            self.banner(
                "The current reply is still running. Stop it before sending another message.".into(),
                true,
            );
            ui(&self.weak, move |w| w.global::<AppState>().set_draft(text.into()));
            return;
        }
        self.submit(text.clone(), text).await;
    }

    async fn submit(&mut self, text: String, display: String) {
        if self.transcript.running {
            self.banner(
                "The current reply is still running. Stop it before sending another message.".into(),
                true,
            );
            ui(&self.weak, move |w| w.global::<AppState>().set_draft(display.into()));
            return;
        }
        let Some(sid) = self.ensure_session().await else {
            ui(&self.weak, move |w| w.global::<AppState>().set_draft(display.into()));
            return;
        };
        // A command's expansion is hidden from the transcript; staged files are not.
        let hidden = text != display;
        // The backend sends its queued images with this turn; show which ones went.
        let mut display = display;
        for a in std::mem::take(&mut self.attachments) {
            display.push_str(&format!("\n{}{}", hermes_core::composer::ATTACHMENT_MARK, a.label()));
        }
        // Staged files travel as @file refs ahead of the message (Electron buildContextText).
        let files = std::mem::take(&mut self.composer.files);
        for f in &files {
            display.push_str(&format!("\n{}{}", hermes_core::composer::ATTACHMENT_MARK, f.label));
        }
        let refs: Vec<String> = files.into_iter().map(|f| f.ref_text).collect();
        let text = hermes_core::composer::with_refs(&refs, &text);
        self.composer.browse_reset();
        self.publish_attachments();
        let ch = self.transcript.push_user(&display);
        self.mark(ch);
        self.flush();

        let client = self.client.clone();
        let commands = self.commands.clone();
        // prompt.submit acks immediately; streaming arrives as events. Don't block the loop.
        tokio::spawn(async move {
            if let Err(e) = client
                .request_with_timeout(
                    "prompt.submit",
                    json!({"session_id": sid, "text": text.clone(), "title_preview": display, "display_kind": if hidden { json!("hidden") } else { Value::Null }}),
                    Duration::from_secs(1800),
                )
                .await
            {
                let _ = commands.send(Cmd::SubmissionFailed { session_id: sid, text: display, message: e.to_string() });
            }
        });
    }

    // ── Attachments ─────────────────────────────────────────────────────────────

    fn publish_attachments(&self) {
        let labels: Vec<SharedString> = self
            .attachments
            .iter()
            .map(|a| a.label().into())
            .chain(self.composer.files.iter().map(|f| f.label.as_str().into()))
            .collect();
        ui(&self.weak, move |w| {
            w.global::<AppState>()
                .set_attachments(ModelRc::new(VecModel::from(labels)));
        });
    }

    fn clear_attachments(&mut self) {
        if !self.attachments.is_empty() || !self.composer.files.is_empty() {
            self.attachments.clear();
            self.composer.files.clear();
            self.publish_attachments();
        }
    }

    /// Ctrl+V: read the desktop clipboard; an image goes to the backend's queue for the
    /// next turn. No image = an ordinary text paste, which the text field already did.
    async fn paste_image(&mut self) {
        let image = match attachments::read_clipboard_image().await {
            Ok(None) => return,
            Ok(Some(image)) => Some(image),
            // No wl-paste/xclip here: let the backend try its own host clipboard.
            Err(ClipboardError::Unavailable(why)) => {
                tracing::info!("desktop clipboard unavailable ({why}); asking the backend");
                None
            }
            Err(e) => return self.banner(format!("Could not paste the image: {e}"), true),
        };
        let Some(sid) = self.ensure_session().await else { return };
        let (method, params) = match &image {
            Some(img) => attachments::attach_bytes_params(&sid, img),
            None => attachments::clipboard_paste_params(&sid),
        };
        let result = self.client.request(method, params).await;
        match result.map(|v| serde_json::from_value::<AttachedImage>(v).unwrap_or_default()) {
            Ok(a) if a.attached => {
                self.attachments.push(a);
                self.publish_attachments();
            }
            // Backend fallback found no image: the paste was text.
            Ok(_) if image.is_none() => {}
            Ok(a) => self.banner(
                format!("The image was not attached: {}", a.message.unwrap_or_default()),
                true,
            ),
            Err(e) => self.banner(format!("Could not attach the image: {e}"), true),
        }
    }

    async fn remove_attachment(&mut self, index: i32) {
        let Some(i) = usize::try_from(index).ok() else { return };
        // Chips list images first, then staged files (which need no backend call to drop).
        if i >= self.attachments.len() {
            if i - self.attachments.len() < self.composer.files.len() {
                self.composer.files.remove(i - self.attachments.len());
                self.publish_attachments();
            }
            return;
        }
        let (Some(sid), Some(path)) = (self.session.clone(), self.attachments[i].path.clone()) else {
            return;
        };
        let (method, params) = attachments::detach_params(&sid, &path);
        match self.client.request(method, params).await {
            Ok(_) => {
                self.attachments.remove(i);
                self.publish_attachments();
            }
            Err(e) => self.banner(format!("Could not remove the attachment: {e}"), true),
        }
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
        // Queued images belong to the chat they were pasted into.
        self.clear_attachments();
        self.composer_reset();
        if self.stored.as_deref() == Some(stored_id.as_str()) {
            return;
        }
        match self
            .client
            .call::<SessionResumeResult>("session.resume", json!({"session_id": stored_id, "source": "desktop"}))
            .await
        {
            Ok(mut r) => {
                self.draft_model = None;
                self.draft_effort = None;
                self.session = Some(r.session_id.clone());
                self.stored = Some(stored_id.clone());
                self.runtime_stores.insert(r.session_id.clone(), stored_id.clone());
                self.project = r
                    .info
                    .project
                    .as_ref()
                    .and_then(|p| p.id.as_ref())
                    .and_then(|id| self.projects.iter().find(|p| &p.id == id))
                    .or_else(|| projects::for_cwd(&self.projects, &r.info.cwd))
                    .cloned();
                self.publish_project();
                self.transcript = Transcript::new();
                let open_requests = r.open_requests.take().unwrap_or_default();
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
                let runtime = r.session_id.clone();
                let draft = self.drafts.get(&stored_id).cloned().unwrap_or_default();
                ui(&self.weak, move |w| {
                    let st = w.global::<AppState>();
                    st.set_active_session(stored_id.into());
                    st.set_draft(draft.into());
                    st.set_runtime_session(runtime.into());
                    st.set_session_title(title.into());
                    st.set_model_label(short_model(&info.model).into());
                    st.set_effort_label(info.reasoning_effort.into());
                    st.set_cwd_label(short_path(&info.cwd).into());
                });
                self.refresh_effort().await;
                self.replay_open_requests(&open_requests);
                self.publish_input();
                if let Some(key) = self.queue_key().map(str::to_owned) {
                    for result in self.side_results.remove(&key).unwrap_or_default() {
                        self.notice(result, false);
                    }
                    self.publish_queue();
                }
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
                // The agent's open_preview / close_preview tools.
                if e.kind == "preview.open" {
                    let url = e.payload.get("url").and_then(Value::as_str).unwrap_or_default();
                    if !url.is_empty() {
                        let _ = self.browser.send(BrowserCmd::Open(url.to_owned()));
                    }
                    return;
                }
                if e.kind == "preview.close" {
                    let _ = self.browser.send(BrowserCmd::Close);
                    return;
                }
                if e.kind == "request.cancel" {
                    let id = e.payload.get("id").and_then(Value::as_str).unwrap_or_default();
                    self.cancel_input(id);
                    if self.pending_approval.as_deref() == Some(id) {
                        self.pending_approval = None;
                        ui(&self.weak, |w| {
                            w.global::<AppState>().set_approval(ApprovalInfo::default())
                        });
                    }
                    return;
                }
                if e.kind == "projects.changed" {
                    self.refresh_projects(false).await;
                    return;
                }
                if matches!(e.kind.as_str(), "background.complete" | "btw.complete") {
                    let sid = e.session_id.clone().unwrap_or_default();
                    let key = self.runtime_stores.get(&sid).cloned().unwrap_or_else(|| sid.clone());
                    let text = e
                        .payload
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("No result text.");
                    let error = e.payload.get("error").and_then(Value::as_str).unwrap_or_default();
                    let task = e.payload.get("task_id").and_then(Value::as_str).unwrap_or("task");
                    let output = format!("{task}: {}", if error.is_empty() { text } else { error });
                    if self.queue_key() == Some(key.as_str()) {
                        self.notice(output, !error.is_empty() || text.starts_with("error:"));
                    } else {
                        self.side_results.entry(key).or_default().push(output);
                        self.banner("Background result ready in its original chat.".into(), false);
                    }
                    return;
                }
                // Only the open session's events touch the transcript.
                if e.session_id.is_some() && e.session_id != self.session {
                    return;
                }
                if e.kind == "session.info" {
                    if e.payload.get("project").is_some() || e.payload.get("cwd").is_some() {
                        let id = e.payload.pointer("/project/id").and_then(Value::as_str);
                        self.project = id
                            .and_then(|id| self.projects.iter().find(|p| p.id == id))
                            .or_else(|| {
                                projects::for_cwd(
                                    &self.projects,
                                    e.payload.get("cwd").and_then(Value::as_str).unwrap_or_default(),
                                )
                            })
                            .cloned();
                        self.publish_project();
                    }
                    let model = e.payload.get("model").and_then(Value::as_str).map(short_model);
                    let effort = e
                        .payload
                        .get("reasoning_effort")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    let cwd = e.payload.get("cwd").and_then(Value::as_str).map(short_path);
                    ui(&self.weak, move |w| {
                        let st = w.global::<AppState>();
                        if let Some(v) = model {
                            st.set_model_label(v.into());
                        }
                        if let Some(v) = effort {
                            st.set_effort_label(v.into());
                        }
                        if let Some(v) = cwd {
                            st.set_cwd_label(v.into());
                        }
                    });
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
                    if e.payload.get("error").is_some_and(|v| !v.is_null())
                        || matches!(
                            e.payload.get("status").and_then(Value::as_str),
                            Some("error" | "failed" | "interrupted" | "cancelled")
                        )
                    {
                        if let Some(key) = self.queue_key().map(str::to_owned) {
                            self.queue_paused.insert(key);
                        }
                    } else {
                        self.drain_queue().await;
                    }
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
            // The agent reads / drives the in-app browser (read_preview, drive_preview).
            "preview.read" => {
                let _ = self.browser.send(BrowserCmd::Read { id, params });
            }
            "preview.act" => {
                let sid = params.get("session_id").and_then(Value::as_str).unwrap_or_default();
                let active =
                    !sid.is_empty() && (self.session.as_deref() == Some(sid) || self.stored.as_deref() == Some(sid));
                let _ = self.browser.send(BrowserCmd::Act { id, params, active });
            }
            other if self.input_request(&id, other, &params) => {}
            other => {
                // Not implemented in this build (tour, window reads, vault logins): decline
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
            .call::<SessionListResult>("session.list", json!({"limit": SESSION_LIST_LIMIT}))
            .await
        {
            Ok(r) => {
                self.all_sessions = r.sessions;
                self.refresh_pins().await;
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
        for g in sidebar_groups(&rows, &self.pinned, now) {
            for (k, &i) in g.indices.iter().enumerate() {
                let r = &rows[i];
                items.push(SessionItem {
                    id: r.id.as_str().into(),
                    title: display_title(r).into(),
                    group: g.label.as_str().into(),
                    first_in_group: k == 0,
                    pinned: self.pinned.contains(&r.id),
                    running: false,
                });
            }
        }
        // Content matches the title filter missed, labelled with their snippet.
        let shown: std::collections::HashSet<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        let extra: Vec<&SessionListRow> = self
            .search_hits
            .iter()
            .filter(|h| !shown.contains(h.id.as_str()))
            .collect();
        for (k, h) in extra.into_iter().enumerate() {
            let title = if h.title.trim().is_empty() {
                display_title(h)
            } else {
                format!(
                    "{} · {}",
                    h.title.trim(),
                    h.preview.chars().take(60).collect::<String>()
                )
            };
            items.push(SessionItem {
                id: h.id.as_str().into(),
                title: title.into(),
                group: "Found in messages".into(),
                first_in_group: k == 0,
                pinned: self.pinned.contains(&h.id),
                running: false,
            });
        }
        self.visible_ids = items.iter().map(|s| s.id.to_string()).collect();
        ui(&self.weak, move |w| {
            w.global::<AppState>()
                .set_sessions(Rc::new(VecModel::from(items)).into());
        });
    }

    async fn refresh_model_label(&mut self) {
        let mut params = json!({});
        if let Some(sid) = &self.session {
            params["session_id"] = json!(sid);
        }
        // The composer label and the picker's tick come from the same answer.
        if let Ok(r) = self.client.call::<ModelOptionsResult>("model.options", params).await {
            let label = short_model(&r.model);
            self.catalog_provider = r.provider.clone();
            self.catalog_model = r.model.clone();
            if !self.catalog.is_empty() {
                self.publish_models();
            }
            ui(&self.weak, move |w| {
                w.global::<AppState>().set_model_label(label.into())
            });
        }
    }

    /// Fetch the catalog and re-render the picker. `force` re-probes every provider
    /// (`model.options` `refresh`), which is what the menu's "Refresh models" asks for;
    /// merely opening the picker uses the backend's cached answer.
    ///
    /// `include_unconfigured` lists configured providers that are signed out (with a note);
    /// `explicit_only` keeps out providers found only through ambient credentials (e.g.
    /// GitHub CLI -> Copilot), as the Electron chat pickers do (`lib/model-options.ts`).
    async fn refresh_models(&mut self, force: bool) {
        let mut params = json!({ "include_unconfigured": true, "explicit_only": true });
        if force {
            params["refresh"] = json!(true);
        }
        if let Some(sid) = &self.session {
            params["session_id"] = json!(sid);
        }
        match self.client.call::<ModelOptionsResult>("model.options", params).await {
            Ok(r) => {
                self.catalog = r.providers.iter().map(|p| p.catalog()).collect();
                self.catalog_provider = r.provider.clone();
                self.catalog_model = r.model.clone();
                self.provider_hints = r
                    .providers
                    .iter()
                    .filter(|p| p.authenticated == Some(false))
                    .map(|p| {
                        let note = p.warning.clone().filter(|w| !w.trim().is_empty());
                        (p.slug.clone(), note.unwrap_or_else(|| "not signed in".to_owned()))
                    })
                    .collect();
                // Snapshot the catalog as judged, so a model that shows up later is not
                // silently hidden by a shortlist chosen before it existed.
                let rows = self.model_rows();
                if let Err(e) = models::seed_known(&mut self.prefs, &rows) {
                    tracing::warn!("could not seed known models: {e}");
                }
                self.publish_models();
            }
            Err(e) => self.banner(format!("Could not load models: {e}"), true),
        }
    }

    /// Catalog rows with the user's hand-typed ids merged in — the set both the picker
    /// and the curation logic work on.
    fn model_rows(&self) -> Vec<CatalogProvider> {
        models::with_customs(&self.catalog, &models::customs(&self.prefs))
    }

    /// The pick the tick marks: the new-chat draft before a session exists, else the
    /// backend's own answer.
    fn current_pick(&self) -> (String, String) {
        match (&self.session, &self.draft_model) {
            (None, Some((p, m))) => (p.clone(), m.clone()),
            _ => (self.catalog_provider.clone(), self.catalog_model.clone()),
        }
    }

    /// Search across the row's id, its `-fast` sibling, and the provider's name + slug —
    /// the same haystack the original folds.
    fn family_matches(&self, provider: &CatalogProvider, family: &models::Family, q: &str) -> bool {
        let hay = format!(
            "{} {} {} {}",
            family.id,
            family.fast_id.clone().unwrap_or_default(),
            provider.name,
            provider.slug
        );
        models::fold_includes(&hay, q)
    }

    /// Re-render the picker menu and the Edit models dialog from the cached catalog plus
    /// the stored curation. No backend call: every list here is local presentation state.
    fn publish_models(&self) {
        let rows = self.model_rows();
        let favorites = models::favorites(&self.prefs);
        let collapsed = models::collapsed_providers(&self.prefs);
        let known = models::known_keys(&self.prefs);
        let stored = models::visible_keys(&self.prefs);
        let visible = models::effective_visible(stored.as_ref(), &rows, known.as_ref());

        let total: usize = rows.iter().map(|p| models::collapse_families(&p.models).len()).sum();
        let shown: usize = rows
            .iter()
            .map(|p| {
                models::collapse_families(&p.models)
                    .iter()
                    .filter(|f| visible.contains(&models::key(&p.slug, &f.id)))
                    .count()
            })
            .sum();

        let empty = if self.catalog.is_empty() {
            "No models are available yet. Configure a provider in Hermes, then reopen this menu.".to_owned()
        } else if !self.model_query.trim().is_empty() || !self.model_edit_query.trim().is_empty() {
            "No models found".to_owned()
        } else {
            "Every model is hidden — open Edit models… to show some.".to_owned()
        };

        let items = self.picker_rows(&rows, &visible, &favorites, &collapsed);
        let edit = self.editor_rows(&rows, &visible, &favorites, &collapsed);
        ui(&self.weak, move |w| {
            let st = w.global::<AppState>();
            st.set_models(Rc::new(VecModel::from(items)).into());
            st.set_model_edit_rows(Rc::new(VecModel::from(edit)).into());
            st.set_model_hidden_count(total.saturating_sub(shown) as i32);
            st.set_model_total_count(total as i32);
            st.set_model_empty_text(empty.into());
        });
    }

    fn picker_rows(
        &self,
        rows: &[CatalogProvider],
        visible: &std::collections::BTreeSet<String>,
        favorites: &[String],
        collapsed: &[String],
    ) -> Vec<ModelItem> {
        let q = models::normalize(&self.model_query);
        let (cur_p, cur_m) = self.current_pick();
        let customs = models::customs(&self.prefs);
        let is_custom =
            |provider: &str, model: &str| customs.iter().any(|c| c.provider == provider && c.model == model);
        let mut items: Vec<ModelItem> = Vec::new();

        // Favorites paint above the provider groups — the whole point of starring one.
        // They ignore the visibility shortlist on purpose: a star IS "always show me this
        // one". A search turns the section off, because a query means "show me matches".
        if q.is_empty() {
            let resolved: Vec<(&CatalogProvider, String)> = favorites
                .iter()
                .filter_map(|k| {
                    let (slug, model) = k.split_once("::")?;
                    let provider = rows.iter().find(|p| p.slug == slug)?;
                    let family = models::collapse_families(&provider.models)
                        .into_iter()
                        .find(|f| f.id == model || f.fast_id.as_deref() == Some(model))?;
                    Some((provider, family.id))
                })
                .collect();
            if !resolved.is_empty() {
                let spans = resolved
                    .iter()
                    .map(|(p, _)| p.slug.as_str())
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    > 1;
                items.push(ModelItem {
                    header: true,
                    provider_name: "Favorites".into(),
                    ..Default::default()
                });
                for (provider, id) in resolved {
                    items.push(ModelItem {
                        provider: provider.slug.as_str().into(),
                        provider_name: provider.name.as_str().into(),
                        model: id.as_str().into(),
                        // Mixed providers name each one once, as a quiet label.
                        hint: if spans {
                            provider.name.as_str().into()
                        } else {
                            SharedString::new()
                        },
                        current: provider.slug == cur_p && id == cur_m,
                        favorite: true,
                        custom: is_custom(&provider.slug, &id),
                        ..Default::default()
                    });
                }
            }
        }

        // Groups, alphabetical by provider name: the backend floats the current provider
        // first, which would reshuffle the list on every switch.
        let mut ordered: Vec<&CatalogProvider> = rows.iter().filter(|p| !p.models.is_empty()).collect();
        ordered.sort_by_key(|a| a.name.to_lowercase());
        let fav_keys: std::collections::BTreeSet<&str> = favorites.iter().map(String::as_str).collect();

        for provider in &ordered {
            let mut families = models::collapse_families(&provider.models);
            // The catalog row is a hint, not the authority: a model the backend is on but
            // the row omits must still render, or the picker has no active row at all.
            if provider.slug == cur_p
                && !cur_m.is_empty()
                && !families
                    .iter()
                    .any(|f| f.id == cur_m || f.fast_id.as_deref() == Some(cur_m.as_str()))
            {
                families.insert(
                    0,
                    models::Family {
                        id: cur_m.clone(),
                        fast_id: None,
                    },
                );
            }
            if families.is_empty() {
                continue;
            }
            let mut wanted: std::collections::BTreeSet<String> = if !q.is_empty() {
                // A search spans every family, regardless of the shortlist.
                families
                    .iter()
                    .filter(|f| self.family_matches(provider, f, &q))
                    .map(|f| f.id.clone())
                    .collect()
            } else {
                families
                    .iter()
                    .filter(|f| visible.contains(&models::key(&provider.slug, &f.id)))
                    .map(|f| f.id.clone())
                    .collect()
            };
            // Keep the active model reachable — but not while searching (a query means
            // "show me the matches").
            if q.is_empty()
                && provider.slug == cur_p
                && let Some(f) = families
                    .iter()
                    .find(|f| f.id == cur_m || f.fast_id.as_deref() == Some(cur_m.as_str()))
            {
                wanted.insert(f.id.clone());
            }
            let listed: Vec<&models::Family> = families
                .iter()
                .filter(|f| {
                    wanted.contains(&f.id)
                        && !(q.is_empty() && fav_keys.contains(models::key(&provider.slug, &f.id).as_str()))
                })
                .collect();
            if listed.is_empty() {
                continue;
            }
            let folded = q.is_empty() && collapsed.iter().any(|c| c == &provider.slug);
            items.push(ModelItem {
                provider: provider.slug.as_str().into(),
                provider_name: provider.name.as_str().into(),
                hint: self
                    .provider_hints
                    .get(&provider.slug)
                    .cloned()
                    .unwrap_or_default()
                    .as_str()
                    .into(),
                header: true,
                collapsed: folded,
                count: listed.len() as i32,
                ..Default::default()
            });
            if folded {
                continue;
            }
            for family in listed {
                items.push(ModelItem {
                    provider: provider.slug.as_str().into(),
                    provider_name: provider.name.as_str().into(),
                    model: family.id.as_str().into(),
                    current: provider.slug == cur_p
                        && (cur_m == family.id || family.fast_id.as_deref() == Some(cur_m.as_str())),
                    favorite: favorites.iter().any(|k| k == &models::key(&provider.slug, &family.id)),
                    custom: is_custom(&provider.slug, &family.id),
                    ..Default::default()
                });
            }
        }

        // A typed id no provider lists is still a model to the backend: offer it as one
        // row per provider, so a slug the catalog lacks is a single click away.
        if items.is_empty()
            && !q.is_empty()
            && let Some(slug) = models::custom_candidate(&self.model_query, rows)
        {
            items.push(ModelItem {
                header: true,
                provider_name: "Custom model".into(),
                ..Default::default()
            });
            for provider in &ordered {
                items.push(ModelItem {
                    provider: provider.slug.as_str().into(),
                    provider_name: provider.name.as_str().into(),
                    model: slug.as_str().into(),
                    add: true,
                    ..Default::default()
                });
            }
        }
        items
    }

    fn editor_rows(
        &self,
        rows: &[CatalogProvider],
        visible: &std::collections::BTreeSet<String>,
        favorites: &[String],
        collapsed: &[String],
    ) -> Vec<EditModelRow> {
        let q = models::normalize(&self.model_edit_query);
        let customs = models::customs(&self.prefs);
        let mut out: Vec<EditModelRow> = Vec::new();
        for provider in rows.iter().filter(|p| !p.models.is_empty()) {
            let families = models::collapse_families(&provider.models);
            if families.is_empty() {
                continue;
            }
            let listed: Vec<&models::Family> = families
                .iter()
                .filter(|f| q.is_empty() || self.family_matches(provider, f, &q))
                .collect();
            if listed.is_empty() {
                continue;
            }
            let on = families
                .iter()
                .filter(|f| visible.contains(&models::key(&provider.slug, &f.id)))
                .count();
            let folded = q.is_empty() && collapsed.iter().any(|c| c == &provider.slug);
            out.push(EditModelRow {
                provider: provider.slug.as_str().into(),
                provider_name: provider.name.as_str().into(),
                hint: self
                    .provider_hints
                    .get(&provider.slug)
                    .cloned()
                    .unwrap_or_default()
                    .as_str()
                    .into(),
                header: true,
                visible: on == families.len(),
                collapsed: folded,
                count: on as i32,
                total: families.len() as i32,
                ..Default::default()
            });
            if folded {
                continue;
            }
            for family in listed {
                out.push(EditModelRow {
                    provider: provider.slug.as_str().into(),
                    provider_name: provider.name.as_str().into(),
                    model: family.id.as_str().into(),
                    visible: visible.contains(&models::key(&provider.slug, &family.id)),
                    favorite: favorites.iter().any(|k| k == &models::key(&provider.slug, &family.id)),
                    custom: customs
                        .iter()
                        .any(|c| c.provider == provider.slug && c.model == family.id),
                    ..Default::default()
                });
            }
        }
        // The dialog offers the same hand-typed id, once the query matches nothing.
        if out.is_empty()
            && !q.is_empty()
            && let Some(slug) = models::custom_candidate(&self.model_edit_query, rows)
        {
            out.push(EditModelRow {
                header: true,
                provider_name: "Custom model".into(),
                ..Default::default()
            });
            for provider in rows.iter().filter(|p| !p.models.is_empty()) {
                out.push(EditModelRow {
                    provider: provider.slug.as_str().into(),
                    provider_name: provider.name.as_str().into(),
                    model: slug.as_str().into(),
                    add: true,
                    ..Default::default()
                });
            }
        }
        out
    }

    /* ------------------------------------------------------- curation handlers */

    /// Persist a new visibility set and re-render both surfaces.
    fn save_visible(&mut self, keys: &std::collections::BTreeSet<String>) {
        let rows = self.model_rows();
        match models::save_visible(&mut self.prefs, keys, &rows) {
            Ok(()) => self.publish_models(),
            Err(e) => self.banner(format!("Could not save the model list: {e}"), true),
        }
    }

    /// Tick / untick one model.
    fn toggle_model(&mut self, provider: &str, model: &str) {
        let rows = self.model_rows();
        let known = models::known_keys(&self.prefs);
        let next = models::toggle_model(
            models::visible_keys(&self.prefs).as_ref(),
            &rows,
            provider,
            model,
            known.as_ref(),
        );
        self.save_visible(&next);
    }

    /// A provider's master switch: all of its models on, or all off.
    fn set_provider_visible(&mut self, provider: &str) {
        let rows = self.model_rows();
        let known = models::known_keys(&self.prefs);
        let stored = models::visible_keys(&self.prefs);
        let families = rows
            .iter()
            .find(|p| p.slug == provider)
            .map(|p| models::collapse_families(&p.models))
            .unwrap_or_default();
        let visible = models::effective_visible(stored.as_ref(), &rows, known.as_ref());
        let on = families
            .iter()
            .filter(|f| visible.contains(&models::key(provider, &f.id)))
            .count();
        // Partial or empty counts as "turn everything on"; a full row turns everything off.
        let target = on != families.len();
        let next = models::set_provider(stored.as_ref(), &rows, provider, target, known.as_ref());
        self.save_visible(&next);
    }

    /// Fold / unfold a provider group in both surfaces.
    fn toggle_provider_collapse(&mut self, provider: &str) {
        if provider.is_empty() {
            return;
        }
        let mut list = models::collapsed_providers(&self.prefs);
        match list.iter().position(|p| p == provider) {
            Some(i) => {
                list.remove(i);
            }
            None => list.push(provider.to_owned()),
        }
        match models::save_collapsed(&mut self.prefs, &list) {
            Ok(()) => self.publish_models(),
            Err(e) => self.banner(format!("Could not save the picker layout: {e}"), true),
        }
    }

    fn toggle_favorite(&mut self, provider: &str, model: &str) {
        let list = models::toggle_favorite(&models::favorites(&self.prefs), provider, model);
        match models::save_favorites(&mut self.prefs, &list) {
            Ok(()) => self.publish_models(),
            Err(e) => self.banner(format!("Could not save favorites: {e}"), true),
        }
    }

    fn add_custom(&mut self, provider: &str, model: &str) {
        let rows = self.model_rows();
        match models::add_custom(&mut self.prefs, &rows, provider, model) {
            Some(Ok(())) => self.publish_models(),
            Some(Err(e)) => self.banner(format!("Could not add “{model}”: {e}"), true),
            None => self.banner("A model id cannot contain spaces.".into(), true),
        }
    }

    fn remove_custom(&mut self, provider: &str, model: &str) {
        match models::remove_custom(&mut self.prefs, provider, model) {
            Ok(()) => self.publish_models(),
            Err(e) => self.banner(format!("Could not remove “{model}”: {e}"), true),
        }
    }

    fn reset_model_visibility(&mut self) {
        let rows = self.model_rows();
        match models::reset_visibility_keeping_customs(&mut self.prefs, &rows) {
            Ok(()) => self.publish_models(),
            Err(e) => self.banner(format!("Could not reset the model list: {e}"), true),
        }
    }

    async fn load_about(&self) {
        let version = self
            .client
            .request("config.get", json!({"key": "version"}))
            .await
            .ok()
            .and_then(|v| v.get("value").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_default();
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
            note: String::new().into(),
            full: matches!(r.kind, "widget" | "fallback") && !widgets::area(r.id).is_empty(),
            area: widgets::area(r.id).into(),
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
