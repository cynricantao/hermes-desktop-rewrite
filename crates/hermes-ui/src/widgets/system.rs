//! Session / gateway / about / plugin / billing widgets → `ui/w-system.slint`.
//!
//! Owned ids: `sessions.archived-list`, `gateway.mode`, `gateway.devices`,
//! `gateway.updates`, `billing.overview`, `keybinds.list`, `plugins.list`,
//! `about.version`, `about.uninstall`.
//!
//! Backend data (payloads captured in `~/.hermes/cache/scratch/settings-api/`, routes checked
//! against `hermes_cli/web_routers/{sessions,dashboard_ui,status}.py`):
//! * `GET /api/sessions?archived=only&order=recent` → `{sessions[], total}` (Electron sends the
//!   same filter); `GET /api/sessions/stats` → `{total, active_store, archived, messages,
//!   by_source}`; `GET /api/sessions/empty/count` → `{count}`; `DELETE /api/sessions/empty`;
//!   `POST /api/sessions/prune` (dry-run first, then `{older_than_days: 90}`).
//! * Restore / delete are fenced (the dashboard's `mutateSessionHttp`): read
//!   `GET /api/sessions/{id}/mutation-snapshot` for `runtime_revision`/`runtime_generation`,
//!   then `PATCH /api/sessions/{id}` body `{archived: false, request_id, expected_revision,
//!   expected_generation, profile}` or `DELETE /api/sessions/{id}?...` with the same identity as
//!   query params — the backend answers 409 `mutation_identity_required` without it.
//! * `GET /api/status` → `{version, release_date, gateway_running, gateway_state}`;
//!   `GET /api/host/identity` → `{ok, protocolVersion, pid, role}`; `POST /api/gateway/restart`.
//! * `GET /api/hermes/update/check` → `{install_method, current_version, behind,
//!   update_available, can_apply, update_command, message}`; `POST /api/hermes/update` applies it.
//! * `GET /api/portal` → `{logged_in, portal_url, subscription_url, account_tier, free_tier,
//!   features[]}`; `GET /api/analytics/usage?days=30` → `{totals, by_model[], period_days}`.
//! * `GET /api/dashboard/plugins` → `[{name, label, description, version, source, tab}]` (hidden
//!   and non-enabled plugins are filtered out server-side); `GET /api/dashboard/plugins/rescan`;
//!   `POST /api/dashboard/plugins/{name}/visibility` body `{hidden}` writes
//!   `dashboard.hidden_plugins` in the profile config, which is where the hidden rows come from.
//! * Keybindings have no backend: [`KEYBINDS`] lists exactly what
//!   `crates/hermes-ui/ui/app-window.slint` and `chat.slint` handle.
//!
//! Action ids this module handles (all `system:<verb>[:<arg>]`):
//! * `sessions-refresh`, `sessions-restore:<id>`, `sessions-delete:<id>`, `sessions-prune`,
//!   `sessions-empty-clean`
//! * `gateway-restart`, `gateway-refresh`, `conn-label:<text>`, `conn-save`, `conn-forget:<id>`,
//!   `conn-use:<id>`
//! * `about-refresh`, `update-apply`
//! * `plugins-refresh`, `plugins-rescan`, `plugin-toggle:<name>:<enable|disable>`
//! * `billing-refresh`
//!
//! Destructive verbs are two-click: the first click arms the exact target (its id is the arm
//! key), the second performs the call. Nothing here deletes a file or quits a process.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use slint::ComponentHandle;

use crate::widgets::{Ctx, Outcome};
use crate::{ArchiveRow, FeatureRow, KeybindRow, PathRow, PluginRow, SavedConnRow, SystemState, UsageModelRow};

/// Widget ids this module draws.
pub(crate) fn owns(id: &str) -> bool {
    matches!(
        id,
        "sessions.archived-list"
            | "gateway.mode"
            | "gateway.devices"
            | "gateway.updates"
            | "billing.overview"
            | "keybinds.list"
            | "plugins.list"
            | "about.version"
            | "about.uninstall"
    )
}

// ── Two-click confirm gate ──────────────────────────────────────────────────────
//
// Keyed by the exact target ("delete:<id>", "prune", …), so a click on one row can never
// confirm another. Kept in the process (the panel re-pushes its rows on every load, so the
// armed state must outlive a widget struct that has none).

const K_EMPTY: &str = "empty-clean";
const K_PRUNE: &str = "prune";
const K_GW_RESTART: &str = "gateway-restart";
const K_UPDATE: &str = "update-apply";

fn armed() -> &'static Mutex<HashSet<String>> {
    static ARMED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    ARMED.get_or_init(|| Mutex::new(HashSet::new()))
}

fn lock_armed() -> std::sync::MutexGuard<'static, HashSet<String>> {
    armed().lock().unwrap_or_else(|e| e.into_inner())
}

fn is_armed(key: &str) -> bool {
    lock_armed().contains(key)
}

fn disarm(key: &str) {
    lock_armed().remove(key);
}

/// First call arms `key` and returns false; the confirming call disarms and returns true.
fn confirm(key: &str) -> bool {
    let mut set = lock_armed();
    if set.remove(key) {
        true
    } else {
        set.insert(key.to_owned());
        false
    }
}

// ── Small helpers ───────────────────────────────────────────────────────────────

/// `&profile=<name>` for paths that already carry a query (the profile route suffix).
fn profile_param(ctx: &Ctx) -> String {
    let q = ctx.profile_query();
    if q.is_empty() {
        String::new()
    } else {
        q.replacen('?', "&", 1)
    }
}

/// Run `f` with the panel's global on the UI thread.
fn state(ctx: &Ctx, f: impl FnOnce(SystemState) + Send + 'static) {
    ctx.ui(move |w| f(w.global::<SystemState>()));
}

fn clear_error(ctx: &Ctx) {
    state(ctx, |s| {
        s.set_busy(false);
        s.set_error(Default::default());
    });
}

fn set_error(ctx: &Ctx, message: impl Into<String>) {
    let message: String = message.into();
    state(ctx, move |s| {
        s.set_busy(false);
        s.set_error(message.into());
    });
}

/// A process-unique request id for the fenced session mutations (no uuid crate in the tree).
fn request_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("rs-desktop-{}-{nanos}", std::process::id())
}

/// `1234567` → `1,234,567`.
fn thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

fn json_i64(v: Option<&Value>) -> i64 {
    v.and_then(Value::as_i64)
        .or_else(|| v.and_then(Value::as_u64).map(|u| u as i64))
        .or_else(|| v.and_then(Value::as_f64).map(|f| f as i64))
        .unwrap_or(0)
}

/// The keybindings this client implements, read from its own event handling:
/// `crates/hermes-ui/ui/app-window.slint` (`keys := FocusScope { capture-key-pressed }`) and
/// `crates/hermes-ui/ui/chat.slint` (composer). No backend, no rebinding store.
const KEYBINDS: &[(&str, &str, &str)] = &[
    ("Window", "Ctrl+N", "New chat"),
    ("Window", "Ctrl+,", "Open Settings"),
    ("Window", "Ctrl+B", "Show / hide the sidebar"),
    ("Window", "Ctrl+Q", "Quit"),
    (
        "Window",
        "Esc",
        "Close Settings, the model picker, the reasoning menu or the project dialog",
    ),
    ("Chat", "Enter", "Send the message"),
    ("Chat", "Shift+Enter", "Insert a new line"),
    ("Chat", "Esc", "Stop the running reply"),
    ("Sessions", "Ctrl+Tab", "Next chat in sidebar order"),
    ("Sessions", "Ctrl+Shift+Tab", "Previous chat in sidebar order"),
    ("Approvals", "Esc", "Deny the approval request"),
    (
        "Approvals",
        "Enter",
        "Allow once (with an empty composer, while an approval is showing)",
    ),
];

// ── Load ────────────────────────────────────────────────────────────────────────

/// Load (or reload) one widget's data into `SystemState`.
pub(crate) async fn load(id: &str, ctx: &Ctx) {
    match id {
        "sessions.archived-list" => load_archived(ctx).await,
        "gateway.mode" => load_gateway(ctx).await,
        "gateway.devices" => load_devices(ctx),
        "gateway.updates" => clear_error(ctx),
        "billing.overview" => load_billing(ctx).await,
        "keybinds.list" => load_keybinds(ctx),
        "plugins.list" => load_plugins(ctx).await,
        "about.version" => load_about(ctx).await,
        "about.uninstall" => load_uninstall(ctx),
        _ => {}
    }
}

async fn load_archived(ctx: &Ctx) {
    let pq = ctx.profile_query();
    let pp = profile_param(ctx);
    let stats = ctx.get_json(&format!("/api/sessions/stats{pq}")).await;
    let empty = ctx.get_json(&format!("/api/sessions/empty/count{pq}")).await;
    let list = ctx
        .get_json(&format!(
            "/api/sessions?archived=only&order=recent&limit=100&offset=0{pp}"
        ))
        .await;

    let mut error: Option<String> = None;
    let summary = match &stats {
        Ok(v) => summary_line(v),
        Err(e) => {
            error = Some(format!("Could not load session stats: {e}"));
            String::new()
        }
    };
    let empty_count = match &empty {
        Ok(v) => json_i64(v.get("count")) as i32,
        Err(e) => {
            error.get_or_insert(format!("Could not count empty sessions: {e}"));
            0
        }
    };
    let rows = match &list {
        Ok(v) => archive_rows(v),
        Err(e) => {
            error.get_or_insert(format!("Could not load archived chats: {e}"));
            Vec::new()
        }
    };
    let (empty_armed, prune_armed) = (is_armed(K_EMPTY), is_armed(K_PRUNE));
    state(ctx, move |s| {
        s.set_busy(false);
        s.set_archived(slint::ModelRc::new(slint::VecModel::from(rows)));
        s.set_sessions_summary(summary.into());
        s.set_empty_count(empty_count);
        s.set_empty_armed(empty_armed);
        s.set_prune_armed(prune_armed);
        s.set_error(error.unwrap_or_default().into());
    });
}

fn summary_line(stats: &Value) -> String {
    let total = json_i64(stats.get("total"));
    let archived = json_i64(stats.get("archived"));
    let messages = json_i64(stats.get("messages"));
    let mut out = format!(
        "{} session{} · {} archived · {} message{}",
        thousands(total),
        if total == 1 { "" } else { "s" },
        thousands(archived),
        thousands(messages),
        if messages == 1 { "" } else { "s" },
    );
    if let Some(by_source) = stats.get("by_source").and_then(Value::as_object)
        && !by_source.is_empty()
    {
        let parts: Vec<String> = by_source
            .iter()
            .map(|(k, v)| format!("{k} {}", json_i64(Some(v))))
            .collect();
        out.push_str(&format!(" · by source: {}", parts.join(", ")));
    }
    out
}

fn archive_rows(list: &Value) -> Vec<ArchiveRow> {
    list.get("sessions")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|r| {
                    let id = r.get("id").and_then(Value::as_str)?.to_owned();
                    let preview = r.get("preview").and_then(Value::as_str).unwrap_or_default().to_owned();
                    let title = r
                        .get("title")
                        .and_then(Value::as_str)
                        .filter(|t| !t.trim().is_empty())
                        .map(str::to_owned)
                        .or_else(|| r.get("display_name").and_then(Value::as_str).map(str::to_owned))
                        .unwrap_or_else(|| {
                            if preview.is_empty() {
                                id.clone()
                            } else {
                                preview.clone()
                            }
                        });
                    let messages = json_i64(r.get("message_count"));
                    let leaf = r
                        .get("cwd")
                        .and_then(Value::as_str)
                        .map(|p| p.rsplit('/').next().unwrap_or(p).to_owned())
                        .filter(|p| !p.is_empty());
                    let hint = match leaf {
                        Some(leaf) => format!("{messages} messages · {leaf}"),
                        None => format!("{messages} messages"),
                    };
                    Some(ArchiveRow {
                        id: id.clone().into(),
                        title: title.into(),
                        preview: preview.into(),
                        hint: hint.into(),
                        busy: false,
                        armed: is_armed(&format!("delete:{id}")),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn load_gateway(ctx: &Ctx) {
    let pq = ctx.profile_query();
    let status = ctx.get_json(&format!("/api/status{pq}")).await;
    let identity = ctx.get_json("/api/host/identity").await;
    let endpoint = format!("{}:{}", ctx.rest.host, ctx.rest.port);

    let mut error: Option<String> = None;
    let (version, released, gw_state) = match &status {
        Ok(v) => {
            let version = v.get("version").and_then(Value::as_str).unwrap_or_default().to_owned();
            let released = v
                .get("release_date")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let running = v.get("gateway_running").and_then(Value::as_bool).unwrap_or(false);
            let state = v
                .get("gateway_state")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            let label = match (running, state) {
                (true, Some(state)) => format!("running ({state})"),
                (true, None) => "running".to_owned(),
                (false, Some(state)) => format!("not running ({state})"),
                (false, None) => "not running".to_owned(),
            };
            (version, released, label)
        }
        Err(e) => {
            error = Some(format!("Could not read the backend status: {e}"));
            (String::new(), String::new(), String::new())
        }
    };
    let (protocol, pid) = match &identity {
        Ok(v) => (v.get("protocolVersion").map(|p| p.to_string()).unwrap_or_default(), {
            let pid = json_i64(v.get("pid"));
            let role = v.get("role").and_then(Value::as_str).unwrap_or_default();
            if pid > 0 {
                format!("{pid} ({role})")
            } else {
                role.to_owned()
            }
        }),
        Err(e) => {
            error.get_or_insert(format!("Could not read the host identity: {e}"));
            (String::new(), String::new())
        }
    };
    let gw_armed = is_armed(K_GW_RESTART);
    state(ctx, move |s| {
        s.set_busy(false);
        s.set_gw_endpoint(endpoint.into());
        s.set_gw_version(version.into());
        s.set_gw_released(released.into());
        s.set_gw_protocol(protocol.into());
        s.set_gw_pid(pid.into());
        s.set_gw_state(gw_state.into());
        s.set_gw_armed(gw_armed);
        s.set_error(error.unwrap_or_default().into());
    });
}

/// Where saved connections live in `prefs.json` — device state, never a backend setting.
const SAVED_KEY: &str = "hermes-desktop-saved-connections";

fn load_saved() -> Vec<SavedConnRow> {
    let prefs = hermes_core::prefs::Prefs::load(hermes_core::prefs::default_path());
    prefs
        .get(SAVED_KEY)
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|r| {
                    let id = r.get("id").and_then(Value::as_str)?.to_owned();
                    let label = r
                        .get("label")
                        .and_then(Value::as_str)
                        .filter(|l| !l.trim().is_empty())
                        .unwrap_or(&id)
                        .to_owned();
                    let host = r.get("host").and_then(Value::as_str).unwrap_or("127.0.0.1");
                    let port = r.get("port").and_then(Value::as_u64).unwrap_or(0);
                    Some(SavedConnRow {
                        id: id.into(),
                        label: label.into(),
                        endpoint: format!("{host}:{port}").into(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn write_saved(rows: &[SavedConnRow]) -> std::io::Result<()> {
    let mut prefs = hermes_core::prefs::Prefs::load(hermes_core::prefs::default_path());
    let list: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "id": r.id.to_string(),
                "label": r.label.to_string(),
                "host": r.endpoint.to_string().rsplit_once(':').map(|(h, _)| h).unwrap_or("127.0.0.1"),
                "port": r.endpoint.to_string().rsplit_once(':').and_then(|(_, p)| p.parse::<u16>().ok()).unwrap_or(0),
            })
        })
        .collect();
    prefs.set(SAVED_KEY, Value::Array(list))
}

fn load_devices(ctx: &Ctx) {
    let endpoint = format!("{}:{}", ctx.rest.host, ctx.rest.port);
    let command = format!("HERMES_DESKTOP_PORT={} hermes-desktop-rs", ctx.rest.port);
    let saved = load_saved();
    let label = last_conn_label();
    state(ctx, move |s| {
        s.set_busy(false);
        s.set_conn_endpoint(endpoint.into());
        s.set_conn_command(command.into());
        s.set_saved(slint::ModelRc::new(slint::VecModel::from(saved)));
        s.set_conn_label(label.into());
        s.set_error(Default::default());
    });
}

/// The label typed into the panel's field (`conn-label` edits land here so `conn-save`, which
/// runs on the controller, sees the same text the user typed).
static CONN_LABEL: OnceLock<Mutex<String>> = OnceLock::new();

fn conn_label_cell() -> &'static Mutex<String> {
    CONN_LABEL.get_or_init(|| Mutex::new(String::new()))
}

fn last_conn_label() -> String {
    conn_label_cell().lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn set_conn_label(value: &str) {
    *conn_label_cell().lock().unwrap_or_else(|e| e.into_inner()) = value.to_owned();
}

fn load_keybinds(ctx: &Ctx) {
    let rows: Vec<KeybindRow> = KEYBINDS
        .iter()
        .map(|(group, keys, label)| KeybindRow {
            group: (*group).into(),
            keys: (*keys).into(),
            label: (*label).into(),
        })
        .collect();
    state(ctx, move |s| {
        s.set_busy(false);
        s.set_keybinds(slint::ModelRc::new(slint::VecModel::from(rows)));
        s.set_error(Default::default());
    });
}

async fn load_plugins(ctx: &Ctx) {
    let list = ctx.get_json("/api/dashboard/plugins").await;
    let config = ctx.config().await;

    let mut error: Option<String> = None;
    let hidden: Vec<String> = config
        .as_ref()
        .ok()
        .and_then(|c| c.pointer("/dashboard/hidden_plugins"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        .unwrap_or_default();
    let mut rows: Vec<PluginRow> = Vec::new();
    match &list {
        Ok(v) => {
            for p in v.as_array().into_iter().flatten() {
                let Some(name) = p.get("name").and_then(Value::as_str) else {
                    continue;
                };
                rows.push(PluginRow {
                    name: name.into(),
                    label: p
                        .get("label")
                        .and_then(Value::as_str)
                        .filter(|l| !l.trim().is_empty())
                        .unwrap_or(name)
                        .into(),
                    description: p.get("description").and_then(Value::as_str).unwrap_or_default().into(),
                    version: p.get("version").and_then(Value::as_str).unwrap_or_default().into(),
                    source: p.get("source").and_then(Value::as_str).unwrap_or_default().into(),
                    tab: p
                        .pointer("/tab/path")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .into(),
                    hidden: false,
                });
            }
        }
        Err(e) => error = Some(format!("Could not list desktop plugins: {e}")),
    }
    // The list endpoint drops hidden plugins, so a hidden one only appears here — from the
    // config key the visibility route writes.
    let known: HashSet<String> = rows.iter().map(|r| r.name.to_string()).collect();
    for name in hidden.iter().filter(|n| !known.contains(*n)) {
        rows.push(PluginRow {
            name: name.as_str().into(),
            label: name.as_str().into(),
            description: String::new().into(),
            version: String::new().into(),
            source: String::new().into(),
            tab: String::new().into(),
            hidden: true,
        });
    }
    let note = format!(
        "{} installed on this backend{}.",
        rows.iter().filter(|r| !r.hidden).count(),
        if rows.iter().any(|r| r.hidden) {
            format!(
                ", {} hidden ({})",
                rows.iter().filter(|r| r.hidden).count(),
                hidden.join(", ")
            )
        } else {
            String::new()
        }
    );
    state(ctx, move |s| {
        s.set_busy(false);
        s.set_plugins(slint::ModelRc::new(slint::VecModel::from(rows)));
        s.set_plugins_note(note.into());
        s.set_error(error.unwrap_or_default().into());
    });
}

async fn load_billing(ctx: &Ctx) {
    let pq = ctx.profile_query();
    let pp = profile_param(ctx);
    let portal = ctx.get_json(&format!("/api/portal{pq}")).await;
    let usage = ctx.get_json(&format!("/api/analytics/usage?days=30{pp}")).await;

    let mut error: Option<String> = None;
    let mut model_rows: Vec<UsageModelRow> = Vec::new();
    let mut tokens = String::new();
    let mut cost = String::new();
    let mut sessions = String::new();
    let mut days = 0i32;
    match &usage {
        Ok(v) => {
            days = json_i64(v.get("period_days")) as i32;
            let totals = v.get("totals").unwrap_or(&Value::Null);
            let input = json_i64(totals.get("total_input"));
            let output = json_i64(totals.get("total_output"));
            let cache = json_i64(totals.get("total_cache_read"));
            let total = input + output;
            tokens = format!(
                "{} in · {} out · {} cache read",
                thousands(input),
                thousands(output),
                thousands(cache)
            );
            cost = totals
                .get("total_estimated_cost")
                .and_then(Value::as_f64)
                .map(|c| format!("${c:.2}"))
                .unwrap_or_else(|| "—".to_owned());
            sessions = format!(
                "{}  ({} total)",
                thousands(json_i64(totals.get("total_sessions"))),
                thousands(total)
            );
            let mut by_model: Vec<(i64, UsageModelRow)> = v
                .get("by_model")
                .and_then(Value::as_array)
                .map(|rows| {
                    rows.iter()
                        .map(|m| {
                            let input = json_i64(m.get("input_tokens"));
                            let output = json_i64(m.get("output_tokens"));
                            (
                                input + output,
                                UsageModelRow {
                                    model: m.get("model").and_then(Value::as_str).unwrap_or("(unknown)").into(),
                                    tokens: format!("{} tok", thousands(input + output)).into(),
                                    cost: m
                                        .get("estimated_cost")
                                        .and_then(Value::as_f64)
                                        .map(|c| format!("${c:.2}"))
                                        .unwrap_or_else(|| "—".to_owned())
                                        .into(),
                                    sessions: format!(
                                        "{} session{}",
                                        json_i64(m.get("sessions")),
                                        if json_i64(m.get("sessions")) == 1 { "" } else { "s" }
                                    )
                                    .into(),
                                },
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            by_model.sort_by_key(|e| std::cmp::Reverse(e.0));
            model_rows = by_model.into_iter().take(5).map(|(_, r)| r).collect();
        }
        Err(e) => error = Some(format!("Could not load usage: {e}")),
    }
    let (logged_in, tier, free, portal_url, subscription, features) = match &portal {
        Ok(v) => {
            let features: Vec<FeatureRow> = v
                .get("features")
                .and_then(Value::as_array)
                .map(|rows| {
                    rows.iter()
                        .filter_map(|f| {
                            Some(FeatureRow {
                                label: f.get("label").and_then(Value::as_str)?.into(),
                                state: f.get("state").and_then(Value::as_str).unwrap_or_default().into(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            (
                v.get("logged_in").and_then(Value::as_bool).unwrap_or(false),
                v.get("account_tier")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                match v.get("free_tier").and_then(Value::as_bool) {
                    Some(true) => "yes".to_owned(),
                    Some(false) => "no".to_owned(),
                    None => String::new(),
                },
                v.get("portal_url")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                v.get("subscription_url")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                features,
            )
        }
        Err(e) => {
            error.get_or_insert(format!("Could not read the portal status: {e}"));
            (
                false,
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                Vec::new(),
            )
        }
    };
    state(ctx, move |s| {
        s.set_busy(false);
        s.set_logged_in(logged_in);
        s.set_account_tier(tier.into());
        s.set_free_tier(free.into());
        s.set_portal_url(portal_url.into());
        s.set_subscription_url(subscription.into());
        s.set_features(slint::ModelRc::new(slint::VecModel::from(features)));
        s.set_usage_tokens(tokens.into());
        s.set_usage_cost(cost.into());
        s.set_usage_sessions(sessions.into());
        s.set_usage_days(days);
        s.set_usage_models(slint::ModelRc::new(slint::VecModel::from(model_rows)));
        s.set_error(error.unwrap_or_default().into());
    });
}

async fn load_about(ctx: &Ctx) {
    let pq = ctx.profile_query();
    let status = ctx.get_json(&format!("/api/status{pq}")).await;
    let check = ctx.get_json(&format!("/api/hermes/update/check{pq}")).await;

    let mut error: Option<String> = None;
    let (backend_version, released) = match &status {
        Ok(v) => (
            v.get("version").and_then(Value::as_str).unwrap_or_default().to_owned(),
            v.get("release_date")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        ),
        Err(e) => {
            error = Some(format!("Could not read the backend version: {e}"));
            (String::new(), String::new())
        }
    };
    let (install_method, update_state, command, available, can_apply) = match &check {
        Ok(v) => (
            v.get("install_method")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            v.get("message").and_then(Value::as_str).unwrap_or_default().to_owned(),
            v.get("update_command")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            v.get("update_available").and_then(Value::as_bool).unwrap_or(false),
            v.get("can_apply").and_then(Value::as_bool).unwrap_or(false),
        ),
        Err(e) => {
            error.get_or_insert(format!("Could not check for updates: {e}"));
            (String::new(), String::new(), String::new(), false, false)
        }
    };
    let client = format!("hermes-desktop-rs {}", env!("CARGO_PKG_VERSION"));
    let update_armed = is_armed(K_UPDATE);
    state(ctx, move |s| {
        s.set_busy(false);
        s.set_checking(false);
        s.set_client_version(client.into());
        s.set_backend_version(backend_version.into());
        s.set_backend_released(released.into());
        s.set_install_method(install_method.into());
        s.set_update_state(update_state.into());
        s.set_update_command(command.into());
        s.set_update_available(available);
        s.set_can_apply(can_apply);
        s.set_update_armed(update_armed);
        s.set_error(error.unwrap_or_default().into());
    });
}

/// The paths this app is installed as on this machine, and the command that removes it.
/// Nothing here is a destructive action: the panel only reports and shows the command.
fn install_paths() -> Vec<PathRow> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut rows: Vec<PathRow> = Vec::new();
    let mut push = |label: &str, path: String| {
        let present = std::path::Path::new(&path).exists();
        rows.push(PathRow {
            label: label.into(),
            path: path.into(),
            present,
        });
    };
    if let Ok(exe) = std::env::current_exe() {
        push("Running binary", exe.display().to_string());
    }
    push("Launcher", format!("{home}/.local/bin/hermes-desktop-rs"));
    push(
        "Desktop entry",
        format!("{home}/.local/share/applications/hermes-desktop-rs.desktop"),
    );
    push("Packaged binary", "/usr/bin/hermes-desktop-rs".to_owned());
    push("Checkout", checkout_dir(&home));
    rows
}

/// The checkout the running binary was built from (nearest ancestor with a `Cargo.toml`).
fn checkout_dir(home: &str) -> String {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        for anc in dir.ancestors().take(6) {
            if anc.join("Cargo.toml").is_file() {
                return anc.display().to_string();
            }
        }
    }
    format!("{home}/Projects/hermes desktop")
}

fn removal_command() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    format!("rm -f {home}/.local/bin/hermes-desktop-rs {home}/.local/share/applications/hermes-desktop-rs.desktop",)
}

fn load_uninstall(ctx: &Ctx) {
    let paths = install_paths();
    let command = removal_command();
    state(ctx, move |s| {
        s.set_busy(false);
        s.set_install_paths(slint::ModelRc::new(slint::VecModel::from(paths)));
        s.set_removal_command(command.into());
        s.set_error(Default::default());
    });
}

// ── Actions ─────────────────────────────────────────────────────────────────────

/// One `system:<verb>[:<arg>]` action from the UI.
pub(crate) async fn action(rest: &str, ctx: &Ctx) -> Outcome {
    let (verb, arg) = rest.split_once(':').unwrap_or((rest, ""));
    match verb {
        // ── Sessions ──
        "sessions-refresh" => {
            load_archived(ctx).await;
            Outcome::None
        }
        "sessions-restore" => archive_session(ctx, arg, false).await,
        "sessions-delete" => delete_session(ctx, arg).await,
        "sessions-prune" => prune(ctx).await,
        "sessions-empty-clean" => clean_empty(ctx).await,
        // ── Gateways ──
        "gateway-refresh" => {
            load_gateway(ctx).await;
            Outcome::None
        }
        "gateway-restart" => restart_gateway(ctx).await,
        "conn-label" => {
            set_conn_label(arg);
            Outcome::None
        }
        "conn-save" => save_connection(ctx),
        "conn-forget" => forget_connection(ctx, arg),
        "conn-use" => {
            let saved = load_saved();
            match saved.iter().find(|r| r.id == arg) {
                Some(row) => Outcome::Notice(format!(
                    "To attach to {}, start the app as: HERMES_DESKTOP_PORT={} hermes-desktop-rs",
                    row.endpoint,
                    row.endpoint.to_string().rsplit_once(':').map(|(_, p)| p).unwrap_or(""),
                )),
                None => Outcome::Error("That saved connection is gone".into()),
            }
        }
        // ── About ──
        "about-refresh" => {
            state(ctx, |s| {
                s.set_checking(true);
                s.set_error(Default::default());
            });
            load_about(ctx).await;
            Outcome::None
        }
        "update-apply" => apply_update(ctx).await,
        // ── Plugins ──
        "plugins-refresh" => {
            load_plugins(ctx).await;
            Outcome::None
        }
        "plugins-rescan" => rescan_plugins(ctx).await,
        "plugin-toggle" => toggle_plugin(ctx, arg).await,
        // ── Billing ──
        "billing-refresh" => {
            load_billing(ctx).await;
            Outcome::None
        }
        "" => Outcome::None,
        other => Outcome::Error(format!("No handler for the “system:{other}” action")),
    }
}

/// Fenced PATCH: the backend needs the session's current revision or it answers 409.
async fn archive_session(ctx: &Ctx, id: &str, archived: bool) -> Outcome {
    if id.is_empty() {
        return Outcome::Error("No session to change".into());
    }
    let pq = ctx.profile_query();
    let snapshot = ctx
        .get_json(&format!(
            "/api/sessions/{}/mutation-snapshot{pq}",
            hermes_core::backend::percent_encode(id)
        ))
        .await;
    let mut body = json!({
        "archived": archived,
        "request_id": request_id(),
    });
    match &snapshot {
        Ok(v) => {
            body["expected_revision"] = json!(json_i64(v.get("runtime_revision")));
            if let Some(g) = v.get("runtime_generation").and_then(Value::as_i64) {
                body["expected_generation"] = json!(g);
            }
        }
        Err(e) => {
            return Outcome::Error(format!("Could not read the chat's revision: {e}"));
        }
    }
    if !ctx.scope.is_empty() {
        body["profile"] = json!(ctx.scope);
    }
    let path = format!("/api/sessions/{}", hermes_core::backend::percent_encode(id));
    match ctx.put_json(&path, &body).await {
        Ok(_) => {
            load_archived(ctx).await;
            Outcome::Notice(if archived {
                "Chat archived".into()
            } else {
                "Chat restored".into()
            })
        }
        Err(e) => {
            load_archived(ctx).await;
            Outcome::Error(format!("Could not change the chat: {e}"))
        }
    }
}

async fn delete_session(ctx: &Ctx, id: &str) -> Outcome {
    if id.is_empty() {
        return Outcome::Error("No session to delete".into());
    }
    let key = format!("delete:{id}");
    if !confirm(&key) {
        // Arm only: re-render the row as "Confirm delete", delete nothing.
        load_archived(ctx).await;
        return Outcome::Notice("Press delete again to remove this chat permanently".into());
    }
    let pq = ctx.profile_query();
    let snapshot = ctx
        .get_json(&format!(
            "/api/sessions/{}/mutation-snapshot{pq}",
            hermes_core::backend::percent_encode(id)
        ))
        .await;
    let mut query = pq.clone();
    let sep = if query.is_empty() { "?" } else { "&" };
    match &snapshot {
        Ok(v) => {
            query.push_str(&format!(
                "{sep}request_id={}&expected_revision={}",
                request_id(),
                json_i64(v.get("runtime_revision"))
            ));
            if let Some(g) = v.get("runtime_generation").and_then(Value::as_i64) {
                query.push_str(&format!("&expected_generation={g}"));
            }
        }
        Err(e) => return Outcome::Error(format!("Could not read the chat's revision: {e}")),
    }
    let path = format!("/api/sessions/{}{query}", hermes_core::backend::percent_encode(id));
    match ctx.delete_json(&path).await {
        Ok(_) => {
            load_archived(ctx).await;
            Outcome::Notice("Chat deleted".into())
        }
        Err(e) => {
            load_archived(ctx).await;
            Outcome::Error(format!("Could not delete the chat: {e}"))
        }
    }
}

/// First click previews with a dry run, the second deletes (mirrors the CLI's confirm).
async fn prune(ctx: &Ctx) -> Outcome {
    let dry = !confirm(K_PRUNE);
    let mut body = json!({ "older_than_days": 90, "dry_run": dry });
    if !ctx.scope.is_empty() {
        body["profile"] = json!(ctx.scope);
    }
    match ctx.post_json("/api/sessions/prune", &body).await {
        Ok(v) => {
            let matched = json_i64(v.get("matched"));
            let removed = json_i64(v.get("removed"));
            let skipped = json_i64(v.get("skipped_open"));
            let note = if dry {
                format!(
                    "{matched} ended chat{} older than 90 days would be deleted ({skipped} still running). Press again to delete them.",
                    if matched == 1 { "" } else { "s" }
                )
            } else {
                format!(
                    "Pruned {removed} chat{} ({skipped} still running were skipped)",
                    if removed == 1 { "" } else { "s" }
                )
            };
            let executed = !dry;
            state(ctx, {
                let note = note.clone();
                move |s| s.set_cleanup_note(note.into())
            });
            load_archived(ctx).await;
            if executed { Outcome::Notice(note) } else { Outcome::None }
        }
        Err(e) => {
            disarm(K_PRUNE);
            load_archived(ctx).await;
            Outcome::Error(format!("Could not prune sessions: {e}"))
        }
    }
}

async fn clean_empty(ctx: &Ctx) -> Outcome {
    if !confirm(K_EMPTY) {
        load_archived(ctx).await;
        return Outcome::Notice("Press the button again to delete the empty chats".into());
    }
    let path = format!("/api/sessions/empty{}", ctx.profile_query());
    match ctx.delete_json(&path).await {
        Ok(v) => {
            let deleted = json_i64(v.get("deleted"));
            load_archived(ctx).await;
            state(ctx, {
                let note = format!("Deleted {deleted} empty chat{}", if deleted == 1 { "" } else { "s" });
                move |s| s.set_cleanup_note(note.into())
            });
            Outcome::Notice(format!(
                "Deleted {deleted} empty chat{}",
                if deleted == 1 { "" } else { "s" }
            ))
        }
        Err(e) => {
            disarm(K_EMPTY);
            load_archived(ctx).await;
            Outcome::Error(format!("Could not delete empty chats: {e}"))
        }
    }
}

async fn restart_gateway(ctx: &Ctx) -> Outcome {
    if !confirm(K_GW_RESTART) {
        load_gateway(ctx).await;
        state(ctx, |s| {
            s.set_gw_note("Press restart again to restart the gateway service".into());
        });
        return Outcome::None;
    }
    let path = format!("/api/gateway/restart{}", ctx.profile_query());
    let result = ctx.post_json(&path, &json!({})).await;
    load_gateway(ctx).await;
    match result {
        Ok(_) => {
            state(ctx, |s| {
                s.set_gw_note("Restart requested; the gateway state above refreshes on the next reload.".into());
            });
            Outcome::Notice("Gateway restart requested".into())
        }
        Err(e) => Outcome::Error(format!("Could not restart the gateway: {e}")),
    }
}

/// Save the current endpoint under the typed label. Device state only — nothing connects.
fn save_connection(ctx: &Ctx) -> Outcome {
    let label = last_conn_label();
    let label = label.trim().to_owned();
    if label.is_empty() {
        return Outcome::Error("Give the connection a name first".into());
    }
    let endpoint = format!("{}:{}", ctx.rest.host, ctx.rest.port);
    let mut rows = load_saved();
    if rows.iter().any(|r| r.endpoint == endpoint.as_str()) {
        return Outcome::Notice(format!("{endpoint} is already saved"));
    }
    rows.push(SavedConnRow {
        id: request_id().into(),
        label: label.as_str().into(),
        endpoint: endpoint.as_str().into(),
    });
    match write_saved(&rows) {
        Ok(()) => {
            load_devices(ctx);
            Outcome::Notice(format!("Saved {endpoint} as “{label}”"))
        }
        Err(e) => Outcome::Error(format!("Could not write prefs.json: {e}")),
    }
}

fn forget_connection(ctx: &Ctx, id: &str) -> Outcome {
    let mut rows = load_saved();
    let before = rows.len();
    rows.retain(|r| r.id != id);
    if rows.len() == before {
        return Outcome::Error("That saved connection is gone".into());
    }
    match write_saved(&rows) {
        Ok(()) => {
            load_devices(ctx);
            Outcome::Notice("Saved connection removed".into())
        }
        Err(e) => Outcome::Error(format!("Could not write prefs.json: {e}")),
    }
}

async fn rescan_plugins(ctx: &Ctx) -> Outcome {
    match ctx.get_json("/api/dashboard/plugins/rescan").await {
        Ok(v) => {
            let count = json_i64(v.get("count"));
            load_plugins(ctx).await;
            Outcome::Notice(format!(
                "Rescanned: {count} desktop plugin{} found",
                if count == 1 { "" } else { "s" }
            ))
        }
        Err(e) => Outcome::Error(format!("Could not rescan plugins: {e}")),
    }
}

async fn toggle_plugin(ctx: &Ctx, arg: &str) -> Outcome {
    let Some((name, mode)) = arg.rsplit_once(':') else {
        return Outcome::Error("Malformed plugin toggle".into());
    };
    let hidden = mode == "disable";
    let path = format!(
        "/api/dashboard/plugins/{}/visibility{}",
        hermes_core::backend::percent_encode(name),
        ctx.profile_query()
    );
    match ctx.post_json(&path, &json!({ "hidden": hidden })).await {
        Ok(_) => {
            load_plugins(ctx).await;
            Outcome::Notice(if hidden {
                format!("{name}: tab hidden")
            } else {
                format!("{name}: tab shown")
            })
        }
        Err(e) => Outcome::Error(format!("Could not change {name}: {e}")),
    }
}

async fn apply_update(ctx: &Ctx) -> Outcome {
    if !confirm(K_UPDATE) {
        load_about(ctx).await;
        state(ctx, |s| {
            s.set_error("Press apply again to run the backend's updater".into());
        });
        return Outcome::None;
    }
    let result = ctx.post_json("/api/hermes/update", &json!({})).await;
    load_about(ctx).await;
    match result {
        Ok(v) => {
            let message = v
                .get("message")
                .or_else(|| v.get("detail"))
                .and_then(Value::as_str)
                .unwrap_or("Update started");
            Outcome::Notice(message.to_owned())
        }
        Err(e) => Outcome::Error(format!("Could not apply the update: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owns_only_its_nine_ids() {
        for id in [
            "sessions.archived-list",
            "gateway.mode",
            "gateway.devices",
            "gateway.updates",
            "billing.overview",
            "keybinds.list",
            "plugins.list",
            "about.version",
            "about.uninstall",
        ] {
            assert!(owns(id), "{id}");
        }
        assert!(!owns("appearance.font"));
        assert!(!owns("model.main"));
    }

    #[test]
    fn thousands_groups_digits() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(1234567), "1,234,567");
        assert_eq!(thousands(-4200), "-4,200");
    }

    #[test]
    fn confirm_needs_two_clicks_and_is_per_target() {
        let a = format!("test-a-{}", std::process::id());
        let b = format!("test-b-{}", std::process::id());
        assert!(!confirm(&a), "first click arms");
        assert!(is_armed(&a));
        assert!(!confirm(&b), "other target unaffected");
        assert!(confirm(&a), "second click confirms");
        assert!(!is_armed(&a), "disarmed after confirming");
        disarm(&b);
        assert!(!is_armed(&b));
    }

    #[test]
    fn summary_line_reports_store_and_sources() {
        let stats = json!({"total": 13, "archived": 1, "messages": 81, "by_source": {"desktop": 10}});
        assert_eq!(
            summary_line(&stats),
            "13 sessions · 1 archived · 81 messages · by source: desktop 10"
        );
        assert_eq!(
            summary_line(&json!({"total": 1, "archived": 0, "messages": 1})),
            "1 session · 0 archived · 1 message"
        );
    }

    #[test]
    fn archive_rows_fall_back_to_the_id() {
        let list = json!({"sessions": [
            {"id": "s1", "title": "Named", "preview": "hello", "message_count": 4, "cwd": "/home/u/proj"},
            {"id": "s2", "title": "", "preview": "", "message_count": 0}
        ]});
        let rows = archive_rows(&list);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].title.to_string(), "Named");
        assert_eq!(rows[0].hint.to_string(), "4 messages · proj");
        assert_eq!(rows[1].title.to_string(), "s2");
        assert_eq!(rows[1].hint.to_string(), "0 messages");
    }

    #[test]
    fn keybinds_match_the_implemented_surface() {
        // Every binding the panel lists must be one the app handles; keep this list short
        // and exact rather than aspirational.
        let labels: Vec<&str> = KEYBINDS.iter().map(|(_, keys, _)| *keys).collect();
        for expected in ["Ctrl+N", "Ctrl+,", "Ctrl+B", "Ctrl+Q", "Ctrl+Tab", "Esc", "Enter"] {
            assert!(labels.contains(&expected), "missing {expected}");
        }
        assert!(!labels.contains(&"Ctrl+K"), "no command palette in this client");
    }
}
