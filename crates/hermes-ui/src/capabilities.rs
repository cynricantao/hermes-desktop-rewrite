//! Capabilities screen controller: its own task, so slow calls (an MCP server test, a
//! cron "run now" that waits for the whole job) never stall chat streaming.
//!
//! Requests and rows come from [`hermes_core::capabilities`]; this module only does the
//! I/O and pushes snapshots to `CapState` on the UI thread.

use std::time::Duration;

use hermes_core::capabilities::{self as caps, Detail, Item, Request, Tab};
use hermes_core::http::Rest;
use serde_json::Value;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel, Weak};
use tokio::sync::mpsc;

use crate::{AppWindow, CapAction, CapItem, CapState};

/// Intents from the Capabilities screen (see `ui/capabilities.slint`).
#[derive(Debug, Clone)]
pub enum CapCmd {
    Refresh,
    Tab(i32),
    Scope(String),
    Toggle {
        id: String,
        on: bool,
    },
    Act {
        id: String,
        action: String,
    },
    Create([String; 3]),
    SaveDetail(String),
    /// The list filter changed (client-side; no request).
    Filter(String),
    /// A slow action spawned by [`Caps::act`] finished.
    Finished {
        notice: String,
        error: bool,
    },
}

const MCP_TEST_TIMEOUT: Duration = Duration::from_secs(60);
/// "Run now" answers only when the job has finished; agent jobs can take a while.
const CRON_RUN_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Start the Capabilities task. Commands sent before the UI opens the screen are fine.
pub fn spawn(rest: Rest, weak: Weak<AppWindow>) -> mpsc::UnboundedSender<CapCmd> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut c = Caps {
        rest,
        weak,
        tx: tx.clone(),
        tab: Tab::Profiles,
        scope: String::new(),
        scopes: Vec::new(),
        items: Vec::new(),
        query: String::new(),
        armed: None,
        detail: None,
    };
    tokio::spawn(async move {
        while let Some(cmd) = rx.recv().await {
            c.handle(cmd).await;
        }
    });
    tx
}

struct Caps {
    rest: Rest,
    weak: Weak<AppWindow>,
    tx: mpsc::UnboundedSender<CapCmd>,
    tab: Tab,
    /// Profile for the profile-scoped tabs ("" = default).
    scope: String,
    scopes: Vec<String>,
    /// Every row of the tab, before the filter.
    items: Vec<Item>,
    query: String,
    /// (row id, action id) waiting for its confirming second click.
    armed: Option<(String, String)>,
    /// What the open detail pane saves to: (kind, row id).
    detail: Option<(Detail, String)>,
}

fn ui(weak: &Weak<AppWindow>, f: impl FnOnce(&CapState) + Send + 'static) {
    let _ = weak.upgrade_in_event_loop(move |w| f(&w.global::<CapState>()));
}

async fn call(rest: &Rest, r: &Request, timeout: Option<Duration>) -> Result<Value, String> {
    let method = r.method.as_str();
    let res = match timeout {
        Some(t) => rest.send_with_timeout(method, &r.path, r.body.as_ref(), t).await,
        None => rest.send(method, &r.path, r.body.as_ref()).await,
    };
    res.map_err(|e| e.to_string())
}

impl Caps {
    /// The profile query for the current tab: only Skills/Tools/MCP/Memory are scoped.
    fn tab_scope(&self) -> &str {
        match self.tab {
            Tab::Skills | Tab::Tools | Tab::Mcp | Tab::Memory => &self.scope,
            _ => "",
        }
    }

    fn notice(&self, text: impl Into<String>, error: bool) {
        let text = text.into();
        ui(&self.weak, move |s| {
            s.set_notice(text.into());
            s.set_notice_error(error);
        });
    }

    fn publish_armed(&self) {
        let armed = self
            .armed
            .as_ref()
            .map(|(id, a)| format!("{id}|{a}"))
            .unwrap_or_default();
        ui(&self.weak, move |s| s.set_armed(armed.into()));
    }

    fn publish_form(&self) {
        let form = caps::form(self.tab);
        let tab = self.tab.index();
        ui(&self.weak, move |s| {
            let (labels, hints, button) = match form {
                Some(f) => (f.fields.to_vec(), f.hints.to_vec(), f.button),
                None => (vec![""; 3], vec![""; 3], ""),
            };
            let strings = |v: Vec<&str>| {
                ModelRc::new(VecModel::from(
                    v.into_iter().map(SharedString::from).collect::<Vec<_>>(),
                ))
            };
            s.set_form_labels(strings(labels));
            s.set_form_hints(strings(hints));
            s.set_form_button(button.into());
            s.set_f1(SharedString::new());
            s.set_f2(SharedString::new());
            s.set_f3(SharedString::new());
            s.set_tab(tab);
            s.set_query(SharedString::new());
            s.set_detail_open(false);
        });
    }

    fn publish_items(&self) {
        let items = caps::filter(&self.items, &self.query);
        ui(&self.weak, move |s| {
            let rows: Vec<CapItem> = items.iter().map(to_cap_item).collect();
            s.set_items(ModelRc::new(VecModel::from(rows)));
        });
    }

    async fn handle(&mut self, cmd: CapCmd) {
        match cmd {
            CapCmd::Refresh => self.load().await,
            CapCmd::Tab(i) => {
                self.tab = Tab::from_index(i);
                self.query.clear();
                self.armed = None;
                self.detail = None;
                self.publish_armed();
                self.publish_form();
                self.notice("", false);
                self.load().await;
            }
            CapCmd::Scope(name) => {
                self.scope = if name == "default" { String::new() } else { name };
                let scope = self.scope.clone();
                ui(&self.weak, move |s| s.set_scope(scope.into()));
                self.load().await;
            }
            CapCmd::Toggle { id, on } => self.toggle(&id, on).await,
            CapCmd::Act { id, action } => self.act(&id, &action).await,
            CapCmd::Create(fields) => self.create(fields).await,
            CapCmd::SaveDetail(text) => self.save_detail(&text).await,
            CapCmd::Filter(q) => {
                self.query = q;
                self.publish_items();
            }
            CapCmd::Finished { notice, error } => {
                self.notice(notice, error);
                self.load().await;
            }
        }
    }

    async fn load(&mut self) {
        ui(&self.weak, |s| s.set_loading(true));
        if self.scopes.is_empty()
            && let Ok(v) = self.rest.get("/api/profiles").await
        {
            self.scopes = caps::parse(Tab::Profiles, &[v]).into_iter().map(|i| i.id).collect();
            let scopes: Vec<SharedString> = self.scopes.iter().map(SharedString::from).collect();
            ui(&self.weak, move |s| s.set_scopes(ModelRc::new(VecModel::from(scopes))));
        }
        let mut responses = Vec::new();
        for (i, r) in caps::list_requests(self.tab, self.tab_scope()).iter().enumerate() {
            match call(&self.rest, r, None).await {
                Ok(v) => responses.push(v),
                // The curator read is optional context on the Memory tab.
                Err(e) if i > 0 => {
                    tracing::warn!(path = %r.path, "capabilities: optional read failed: {e}");
                    responses.push(Value::Null);
                }
                Err(e) => {
                    self.items.clear();
                    self.publish_items();
                    self.notice(format!("Could not load {}: {e}", self.tab.label()), true);
                    ui(&self.weak, |s| s.set_loading(false));
                    return;
                }
            }
        }
        self.items = caps::parse(self.tab, &responses);
        // A list built after a profile change also refreshes the profile pills.
        if self.tab == Tab::Profiles {
            self.scopes = self.items.iter().map(|i| i.id.clone()).collect();
            let scopes: Vec<SharedString> = self.scopes.iter().map(SharedString::from).collect();
            ui(&self.weak, move |s| s.set_scopes(ModelRc::new(VecModel::from(scopes))));
        }
        self.publish_items();
        ui(&self.weak, |s| s.set_loading(false));
    }

    fn title_of(&self, id: &str) -> String {
        self.items
            .iter()
            .find(|i| i.id == id)
            .map(|i| i.title.clone())
            .unwrap_or_else(|| id.to_owned())
    }

    async fn toggle(&mut self, id: &str, on: bool) {
        let Some(r) = caps::toggle_request(self.tab, id, on, self.tab_scope()) else {
            return;
        };
        match call(&self.rest, &r, None).await {
            Ok(resp) => {
                let installing = resp.get("post_setup_started").and_then(Value::as_str).unwrap_or("");
                if installing.is_empty() {
                    self.notice("", false);
                } else {
                    self.notice(format!("Installing {installing} in the background."), false);
                }
            }
            Err(e) => self.notice(format!("Could not change {}: {e}", self.title_of(id)), true),
        }
        // Read back the backend's state (and undo the switch's own flip after a failure).
        self.load().await;
    }

    async fn act(&mut self, id: &str, action: &str) {
        if let Some(kind) = caps::detail_for(self.tab, action) {
            return self.open_detail(kind, id).await;
        }
        let Some(spec) = self
            .items
            .iter()
            .find(|i| i.id == id)
            .and_then(|i| i.actions.iter().find(|a| a.id == action).copied())
        else {
            return;
        };
        let title = self.title_of(id);
        let key = (id.to_owned(), action.to_owned());
        if spec.confirm && self.armed.as_ref() != Some(&key) {
            self.armed = Some(key);
            self.publish_armed();
            let cost = if action == "run" {
                " This starts an agent run that uses your model."
            } else {
                ""
            };
            return self.notice(
                format!(
                    "Click “Confirm {}” to {} {title}.{cost}",
                    spec.label.to_lowercase(),
                    spec.label.to_lowercase()
                ),
                false,
            );
        }
        self.armed = None;
        self.publish_armed();
        let Some(r) = caps::action_request(self.tab, id, action, self.tab_scope()) else {
            return;
        };
        let slow = match (self.tab, action) {
            (Tab::Mcp, "test") => Some(MCP_TEST_TIMEOUT),
            (Tab::Cron, "run") => Some(CRON_RUN_TIMEOUT),
            _ => None,
        };
        if let Some(timeout) = slow {
            self.notice(
                format!("{title}: {}…", if action == "test" { "testing" } else { "running" }),
                false,
            );
            let (rest, tx, tab, action) = (self.rest.clone(), self.tx.clone(), self.tab, action.to_owned());
            tokio::spawn(async move {
                let (notice, error) = match call(&rest, &r, Some(timeout)).await {
                    Ok(resp) => (caps::action_notice(tab, &action, &title, &resp), false),
                    Err(e) => (format!("{title}: {e}"), true),
                };
                let _ = tx.send(CapCmd::Finished { notice, error });
            });
            return;
        }
        match call(&self.rest, &r, None).await {
            Ok(resp) => self.notice(caps::action_notice(self.tab, action, &title, &resp), false),
            Err(e) => self.notice(format!("{title}: {e}"), true),
        }
        if self.tab == Tab::Profiles {
            self.scopes.clear();
        }
        self.load().await;
    }

    async fn open_detail(&mut self, kind: Detail, id: &str) {
        let title = match kind {
            Detail::Soul => format!("SOUL.md — {id}"),
            Detail::Skill => format!("SKILL.md — {id}"),
            Detail::Prompt => format!("Prompt — {}", self.title_of(id)),
            Detail::Rename => format!("New name for profile {id}"),
        };
        let text = match caps::detail_request(kind, id, self.tab_scope()) {
            None => id.to_owned(),
            Some(r) => match call(&self.rest, &r, None).await {
                Ok(v) => caps::detail_text(kind, &v),
                Err(e) => return self.notice(format!("Could not open {title}: {e}"), true),
            },
        };
        self.detail = Some((kind, id.to_owned()));
        let single = kind == Detail::Rename;
        ui(&self.weak, move |s| {
            s.set_detail_title(title.into());
            s.set_detail_text(text.into());
            s.set_detail_single_line(single);
            s.set_detail_busy(false);
            s.set_detail_open(true);
        });
        self.notice("", false);
    }

    async fn save_detail(&mut self, text: &str) {
        let Some((kind, id)) = self.detail.clone() else { return };
        let r = match caps::save_detail_request(kind, &id, text, self.tab_scope()) {
            Ok(r) => r,
            Err(e) => return self.notice(e, true),
        };
        ui(&self.weak, |s| s.set_detail_busy(true));
        let res = call(&self.rest, &r, None).await;
        ui(&self.weak, |s| s.set_detail_busy(false));
        match res {
            Ok(_) => {
                self.detail = None;
                ui(&self.weak, |s| s.set_detail_open(false));
                self.notice(
                    match kind {
                        Detail::Rename => format!("Renamed {id} to {}.", text.trim()),
                        _ => "Saved.".to_owned(),
                    },
                    false,
                );
                if kind == Detail::Rename {
                    self.scopes.clear();
                }
                self.load().await;
            }
            Err(e) => self.notice(format!("Could not save: {e}"), true),
        }
    }

    async fn create(&mut self, fields: [String; 3]) {
        let f = [fields[0].as_str(), fields[1].as_str(), fields[2].as_str()];
        let r = match caps::create_request(self.tab, f, self.tab_scope()) {
            Ok(r) => r,
            Err(e) => return self.notice(e, true),
        };
        match call(&self.rest, &r, None).await {
            Ok(resp) => {
                let name = f
                    .iter()
                    .copied()
                    .find(|x| !x.trim().is_empty())
                    .unwrap_or("item")
                    .trim()
                    .to_owned();
                let mut notice = format!("Created {name}.");
                // A profile can be created while its model write is rejected; say why.
                if let Some(err) = resp
                    .get("model_error")
                    .and_then(Value::as_str)
                    .filter(|e| !e.is_empty())
                {
                    notice.push_str(&format!(" Model not set: {err}"));
                }
                self.notice(notice, false);
                ui(&self.weak, |s| {
                    s.set_f1(SharedString::new());
                    s.set_f2(SharedString::new());
                    s.set_f3(SharedString::new());
                });
                if self.tab == Tab::Profiles {
                    self.scopes.clear();
                }
                self.load().await;
            }
            Err(e) => self.notice(format!("Could not create: {e}"), true),
        }
    }
}

fn to_cap_item(i: &Item) -> CapItem {
    let actions: Vec<CapAction> = i
        .actions
        .iter()
        .map(|a| CapAction {
            id: a.id.into(),
            label: a.label.into(),
            confirm: a.confirm,
        })
        .collect();
    CapItem {
        id: i.id.as_str().into(),
        title: i.title.as_str().into(),
        subtitle: i.subtitle.as_str().into(),
        badge: i.badge.as_str().into(),
        has_toggle: i.toggle.is_some(),
        on: i.toggle.unwrap_or(false),
        actions: ModelRc::new(VecModel::from(actions)),
    }
}
