//! Capabilities screen: profiles, skills, toolsets, MCP servers, memory and scheduled
//! jobs, managed over the backend's existing REST routes (the same ones the Electron
//! desktop calls from `apps/desktop/src/api/{profiles,skills,toolsets,mcp,cron,system}.ts`).
//!
//! Pure: this module only builds requests and turns responses into display rows. The UI
//! controller performs the I/O, so every contract detail here is unit-tested without a
//! backend.

use serde_json::{Value, json};

use crate::backend::percent_encode;
use crate::http::profile_query;

/// The six tabs of the Capabilities screen, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Profiles,
    Skills,
    Tools,
    Mcp,
    Memory,
    Cron,
}

impl Tab {
    pub const ALL: [Tab; 6] = [Tab::Profiles, Tab::Skills, Tab::Tools, Tab::Mcp, Tab::Memory, Tab::Cron];

    pub fn from_index(i: i32) -> Tab {
        usize::try_from(i)
            .ok()
            .and_then(|i| Self::ALL.get(i).copied())
            .unwrap_or(Tab::Profiles)
    }

    pub fn index(self) -> i32 {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0) as i32
    }

    pub fn label(self) -> &'static str {
        match self {
            Tab::Profiles => "Profiles",
            Tab::Skills => "Skills",
            Tab::Tools => "Tools",
            Tab::Mcp => "MCP",
            Tab::Memory => "Memory",
            Tab::Cron => "Scheduled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Patch => "PATCH",
            Method::Delete => "DELETE",
        }
    }
}

/// One REST call: method, path (query included) and optional JSON body.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub method: Method,
    pub path: String,
    pub body: Option<Value>,
}

fn req(method: Method, path: impl Into<String>, body: Option<Value>) -> Request {
    Request {
        method,
        path: path.into(),
        body,
    }
}

/// A row button. `confirm` actions need a second click (they delete data or start an
/// agent run that spends model calls).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Action {
    pub id: &'static str,
    pub label: &'static str,
    pub confirm: bool,
}

const fn act(id: &'static str, label: &'static str, confirm: bool) -> Action {
    Action { id, label, confirm }
}

/// A display row. `toggle` is `Some(on)` when the row has an enable switch.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Item {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub badge: String,
    pub toggle: Option<bool>,
    pub actions: Vec<Action>,
}

/// Memory-tab row ids that are not provider names.
pub const MEMORY_FILE: &str = "file:memory";
pub const USER_FILE: &str = "file:user";
pub const CURATOR: &str = "curator";
/// Row id for "no external provider" (the built-in files only).
pub const BUILTIN_PROVIDER: &str = "provider:";

/// `?profile=<name>` that always names the profile. Destructive routes (memory reset,
/// curator run) answer 400 to an unnamed profile once the backend hosts several, so the
/// default profile must be spelled out there (`hermes_cli/web_routers/_common.py`).
fn explicit_query(profile: &str) -> String {
    let name = if profile.is_empty() { "default" } else { profile };
    format!("?profile={}", percent_encode(name))
}

fn seg(s: &str) -> String {
    percent_encode(s)
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

fn b(v: &Value, key: &str) -> Option<bool> {
    v.get(key).and_then(Value::as_bool)
}

fn first_line(text: &str, max: usize) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    if line.chars().count() > max {
        let cut: String = line.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    } else {
        line.to_owned()
    }
}

/// Human file size ("0 B", "812 B", "4.2 KB").
pub fn human_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

// ── Listing ─────────────────────────────────────────────────────────────────────

/// The reads that fill a tab. `profile` scopes skills/tools/MCP/memory ("" = default).
/// Memory needs two reads (memory status, then curator status).
pub fn list_requests(tab: Tab, profile: &str) -> Vec<Request> {
    let q = profile_query(profile);
    match tab {
        Tab::Profiles => vec![req(Method::Get, "/api/profiles", None)],
        Tab::Skills => vec![req(Method::Get, format!("/api/skills{q}"), None)],
        Tab::Tools => vec![req(Method::Get, format!("/api/tools/toolsets{q}"), None)],
        Tab::Mcp => vec![req(Method::Get, format!("/api/mcp/servers{q}"), None)],
        Tab::Memory => vec![
            req(Method::Get, format!("/api/memory{q}"), None),
            req(Method::Get, format!("/api/curator{q}"), None),
        ],
        // The list endpoint defaults to every profile; ask for that explicitly.
        Tab::Cron => vec![req(Method::Get, "/api/cron/jobs?profile=all", None)],
    }
}

/// Turn the responses of [`list_requests`] (same order) into rows.
pub fn parse(tab: Tab, responses: &[Value]) -> Vec<Item> {
    let first = responses.first().unwrap_or(&Value::Null);
    match tab {
        Tab::Profiles => parse_profiles(first),
        Tab::Skills => parse_skills(first),
        Tab::Tools => parse_toolsets(first),
        Tab::Mcp => parse_mcp(first),
        Tab::Memory => parse_memory(first, responses.get(1)),
        Tab::Cron => parse_cron(first),
    }
}

fn array<'a>(v: &'a Value, key: Option<&str>) -> &'a [Value] {
    let v = match key {
        Some(k) => v.get(k).unwrap_or(v),
        None => v,
    };
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}

fn parse_profiles(v: &Value) -> Vec<Item> {
    array(v, Some("profiles"))
        .iter()
        .filter_map(|p| {
            let name = s(p, "name");
            if name.is_empty() {
                return None;
            }
            let is_default = b(p, "is_default").unwrap_or(name == "default");
            let label = [s(p, "bot_title"), s(p, "display_name")]
                .into_iter()
                .find(|l| !l.trim().is_empty())
                .unwrap_or(if is_default { "Default" } else { name });
            let mut sub = Vec::new();
            if !s(p, "model").is_empty() {
                sub.push(s(p, "model").to_owned());
            }
            if let Some(n) = p.get("skill_count").and_then(Value::as_u64) {
                sub.push(format!("{n} skills"));
            }
            if !s(p, "description").is_empty() {
                sub.push(first_line(s(p, "description"), 80));
            }
            let mut actions = vec![act("soul", "Soul", false), act("rename", "Rename", false)];
            if !is_default {
                actions.push(act("delete", "Delete", true));
            }
            Some(Item {
                id: name.to_owned(),
                title: if label == name {
                    name.to_owned()
                } else {
                    format!("{label} ({name})")
                },
                subtitle: sub.join(" · "),
                badge: if b(p, "gateway_running") == Some(true) {
                    "gateway on".into()
                } else if is_default {
                    "default".into()
                } else {
                    String::new()
                },
                toggle: None,
                actions,
            })
        })
        .collect()
}

fn parse_skills(v: &Value) -> Vec<Item> {
    let mut items: Vec<Item> = array(v, None)
        .iter()
        .filter_map(|sk| {
            let name = s(sk, "name");
            if name.is_empty() {
                return None;
            }
            let mut sub = Vec::new();
            if !s(sk, "category").is_empty() {
                sub.push(s(sk, "category").to_owned());
            }
            if !s(sk, "description").is_empty() {
                sub.push(first_line(s(sk, "description"), 110));
            }
            Some(Item {
                id: name.to_owned(),
                title: name.to_owned(),
                subtitle: sub.join(" · "),
                badge: s(sk, "provenance").to_owned(),
                toggle: Some(b(sk, "enabled").unwrap_or(true)),
                actions: vec![act("view", "View", false)],
            })
        })
        .collect();
    items.sort_by_key(|a| a.title.to_lowercase());
    items
}

fn parse_toolsets(v: &Value) -> Vec<Item> {
    array(v, None)
        .iter()
        .filter_map(|t| {
            let name = s(t, "name");
            if name.is_empty() {
                return None;
            }
            let label = if s(t, "label").is_empty() { name } else { s(t, "label") };
            let tools = t.get("tools").and_then(Value::as_array).map_or(0, Vec::len);
            let mut sub = Vec::new();
            if !s(t, "description").is_empty() {
                sub.push(first_line(s(t, "description"), 100));
            }
            sub.push(format!("{tools} tools"));
            Some(Item {
                id: name.to_owned(),
                title: label.to_owned(),
                subtitle: sub.join(" · "),
                badge: if b(t, "configured") == Some(false) {
                    "needs setup".into()
                } else {
                    String::new()
                },
                toggle: Some(b(t, "enabled").unwrap_or(false)),
                actions: vec![],
            })
        })
        .collect()
}

fn parse_mcp(v: &Value) -> Vec<Item> {
    array(v, Some("servers"))
        .iter()
        .filter_map(|m| {
            let name = s(m, "name");
            if name.is_empty() {
                return None;
            }
            let target = if !s(m, "url").is_empty() {
                s(m, "url").to_owned()
            } else {
                let args: Vec<&str> = m
                    .get("args")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                std::iter::once(s(m, "command"))
                    .chain(args)
                    .filter(|p| !p.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            let from_plugin = s(m, "source") == "plugin";
            let mut actions = vec![act("test", "Test", false)];
            if !from_plugin {
                actions.push(act("remove", "Remove", true));
            }
            Some(Item {
                id: name.to_owned(),
                title: name.to_owned(),
                subtitle: format!("{} · {}", s(m, "transport"), target),
                badge: if from_plugin {
                    format!("plugin {}", s(m, "plugin"))
                } else {
                    String::new()
                },
                // Plugin-provided servers cannot be modified (the backend answers 409).
                toggle: (!from_plugin).then(|| b(m, "enabled").unwrap_or(true)),
                actions,
            })
        })
        .collect()
}

fn parse_memory(mem: &Value, curator: Option<&Value>) -> Vec<Item> {
    let active = s(mem, "active");
    let files = mem.get("builtin_files").unwrap_or(&Value::Null);
    let size = |k: &str| files.get(k).and_then(Value::as_u64).unwrap_or(0);
    let mut items = vec![
        Item {
            id: MEMORY_FILE.into(),
            title: "MEMORY.md".into(),
            subtitle: format!("Agent notes · {}", human_size(size("memory"))),
            badge: "built-in".into(),
            toggle: None,
            actions: vec![act("reset", "Reset", true)],
        },
        Item {
            id: USER_FILE.into(),
            title: "USER.md".into(),
            subtitle: format!("User profile · {}", human_size(size("user"))),
            badge: "built-in".into(),
            toggle: None,
            actions: vec![act("reset", "Reset", true)],
        },
        Item {
            id: BUILTIN_PROVIDER.into(),
            title: "Built-in memory only".into(),
            subtitle: "No external memory provider".into(),
            badge: if active.is_empty() {
                "active".into()
            } else {
                String::new()
            },
            toggle: None,
            actions: if active.is_empty() {
                vec![]
            } else {
                vec![act("use", "Use", false)]
            },
        },
    ];
    for p in array(mem, Some("providers")) {
        let name = s(p, "name");
        if name.is_empty() {
            continue;
        }
        let is_active = name == active;
        let available = b(p, "available").unwrap_or(false);
        let mut sub = vec![if s(p, "description").is_empty() {
            "Memory provider".to_owned()
        } else {
            first_line(s(p, "description"), 100)
        }];
        if !available {
            sub.push("not available".into());
        }
        items.push(Item {
            id: format!("provider:{name}"),
            title: name.to_owned(),
            subtitle: sub.join(" · "),
            badge: if is_active { "active".into() } else { String::new() },
            toggle: None,
            actions: if is_active || !available {
                vec![]
            } else {
                vec![act("use", "Use", false)]
            },
        });
    }
    if let Some(c) = curator.filter(|c| c.is_object()) {
        let mut sub = vec!["Background skill maintenance".to_owned()];
        if let Some(h) = c.get("interval_hours").and_then(Value::as_f64) {
            sub.push(format!("every {h}h"));
        }
        let last = c.get("last_run_at").map(text_of).unwrap_or_default();
        sub.push(if last.is_empty() {
            "never run".into()
        } else {
            format!("last run {last}")
        });
        items.push(Item {
            id: CURATOR.into(),
            title: "Skill curator".into(),
            subtitle: sub.join(" · "),
            badge: if b(c, "enabled") == Some(false) {
                "disabled".into()
            } else {
                String::new()
            },
            toggle: Some(!b(c, "paused").unwrap_or(false)),
            actions: vec![act("run", "Run now", true)],
        });
    }
    items
}

fn text_of(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn parse_cron(v: &Value) -> Vec<Item> {
    array(v, Some("jobs"))
        .iter()
        .filter_map(|j| {
            let id = s(j, "id");
            if id.is_empty() {
                return None;
            }
            let state = s(j, "state");
            let paused = state == "paused" || b(j, "enabled") == Some(false);
            let schedule = match s(j, "schedule_display") {
                "" => j
                    .get("schedule")
                    .and_then(|s| s.get("display"))
                    .map(text_of)
                    .unwrap_or_default(),
                d => d.to_owned(),
            };
            let mut sub = vec![schedule];
            let next = j.get("next_run_at").map(text_of).unwrap_or_default();
            if !next.is_empty() && !paused {
                sub.push(format!("next {next}"));
            }
            let last_status = j.get("last_status").map(text_of).unwrap_or_default();
            if !last_status.is_empty() {
                sub.push(format!("last {last_status}"));
            }
            if !s(j, "deliver").is_empty() && s(j, "deliver") != "local" {
                sub.push(format!("to {}", s(j, "deliver")));
            }
            let title = match s(j, "name") {
                "" => first_line(s(j, "prompt"), 60),
                n => n.to_owned(),
            };
            Some(Item {
                id: id.to_owned(),
                title: if title.is_empty() { id.to_owned() } else { title },
                subtitle: sub
                    .into_iter()
                    .filter(|x| !x.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · "),
                badge: if state.is_empty() {
                    String::new()
                } else {
                    state.to_owned()
                },
                toggle: Some(!paused),
                actions: vec![
                    act("prompt", "Prompt", false),
                    act("run", "Run now", true),
                    act("delete", "Delete", true),
                ],
            })
        })
        .collect()
}

/// Rows whose title, subtitle, badge or id contain every word of `query`
/// (case-insensitive). An empty query keeps every row.
pub fn filter(items: &[Item], query: &str) -> Vec<Item> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    items
        .iter()
        .filter(|i| {
            let hay = format!("{} {} {} {}", i.title, i.subtitle, i.badge, i.id).to_lowercase();
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .cloned()
        .collect()
}

// ── Mutations ───────────────────────────────────────────────────────────────────

/// The switch on a row. `None` when the tab has no switch for that row.
pub fn toggle_request(tab: Tab, id: &str, on: bool, profile: &str) -> Option<Request> {
    let q = profile_query(profile);
    match tab {
        Tab::Skills => Some(req(
            Method::Put,
            format!("/api/skills/toggle{q}"),
            Some(json!({"name": id, "enabled": on})),
        )),
        Tab::Tools => Some(req(
            Method::Put,
            format!("/api/tools/toolsets/{}{q}", seg(id)),
            Some(json!({"enabled": on})),
        )),
        Tab::Mcp => Some(req(
            Method::Put,
            format!("/api/mcp/servers/{}/enabled{q}", seg(id)),
            Some(json!({"enabled": on})),
        )),
        Tab::Memory if id == CURATOR => Some(req(
            Method::Put,
            format!("/api/curator/paused{q}"),
            Some(json!({"paused": !on})),
        )),
        Tab::Cron => Some(req(
            Method::Post,
            format!("/api/cron/jobs/{}/{}", seg(id), if on { "resume" } else { "pause" }),
            None,
        )),
        _ => None,
    }
}

/// A row button that acts immediately (after its confirmation, when it has one).
/// Buttons that open the detail pane (`view`, `soul`, `prompt`, `rename`) are not here.
pub fn action_request(tab: Tab, id: &str, action: &str, profile: &str) -> Option<Request> {
    let q = profile_query(profile);
    match (tab, action) {
        (Tab::Profiles, "delete") if id != "default" => {
            Some(req(Method::Delete, format!("/api/profiles/{}", seg(id)), None))
        }
        // Connects for real (spawns a stdio server or calls the URL), lists tools, disconnects.
        (Tab::Mcp, "test") => Some(req(Method::Post, format!("/api/mcp/servers/{}/test{q}", seg(id)), None)),
        (Tab::Mcp, "remove") => Some(req(Method::Delete, format!("/api/mcp/servers/{}{q}", seg(id)), None)),
        (Tab::Memory, "reset") => {
            let target = match id {
                MEMORY_FILE => "memory",
                USER_FILE => "user",
                _ => return None,
            };
            Some(req(
                Method::Post,
                format!("/api/memory/reset{}", explicit_query(profile)),
                Some(json!({"target": target})),
            ))
        }
        (Tab::Memory, "use") => {
            let provider = id.strip_prefix("provider:")?;
            Some(req(
                Method::Put,
                format!("/api/memory/provider{q}"),
                Some(json!({"provider": provider})),
            ))
        }
        (Tab::Memory, "run") if id == CURATOR => Some(req(
            Method::Post,
            format!("/api/curator/run{}", explicit_query(profile)),
            None,
        )),
        (Tab::Cron, "run") => Some(req(Method::Post, format!("/api/cron/jobs/{}/trigger", seg(id)), None)),
        (Tab::Cron, "delete") => Some(req(Method::Delete, format!("/api/cron/jobs/{}", seg(id)), None)),
        _ => None,
    }
}

/// One-line result of a successful action, for the screen's notice line.
pub fn action_notice(tab: Tab, action: &str, title: &str, resp: &Value) -> String {
    match (tab, action) {
        (Tab::Mcp, "test") => {
            if b(resp, "ok") == Some(false) {
                let err = s(resp, "error");
                format!(
                    "{title}: test failed{}",
                    if err.is_empty() {
                        String::new()
                    } else {
                        format!(" — {err}")
                    }
                )
            } else {
                let n = resp.get("tools").and_then(Value::as_array).map_or(0, Vec::len);
                format!("{title}: connected, {n} tools")
            }
        }
        (Tab::Memory, "reset") => {
            let deleted: Vec<&str> = array(resp, Some("deleted")).iter().filter_map(Value::as_str).collect();
            if deleted.is_empty() {
                format!("{title} was already empty.")
            } else {
                format!("Deleted {}.", deleted.join(", "))
            }
        }
        (Tab::Memory, "use") => format!("Memory provider set to {title}."),
        (Tab::Memory, "run") => "Curator run started in the background.".into(),
        (Tab::Cron, "run") => format!(
            "{title}: run finished ({}).",
            text_of(resp.get("last_status").unwrap_or(&Value::Null))
        ),
        (_, "delete") | (_, "remove") => format!("Removed {title}."),
        _ => format!("{title}: done."),
    }
}

/// What the detail pane edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detail {
    /// A profile's SOUL.md (editable).
    Soul,
    /// A skill's SKILL.md (editable).
    Skill,
    /// A cron job's prompt (editable).
    Prompt,
    /// A profile's new name.
    Rename,
}

pub fn detail_for(tab: Tab, action: &str) -> Option<Detail> {
    match (tab, action) {
        (Tab::Profiles, "soul") => Some(Detail::Soul),
        (Tab::Profiles, "rename") => Some(Detail::Rename),
        (Tab::Skills, "view") => Some(Detail::Skill),
        (Tab::Cron, "prompt") => Some(Detail::Prompt),
        _ => None,
    }
}

/// The read that fills the detail pane (`None` = nothing to fetch, e.g. Rename).
pub fn detail_request(detail: Detail, id: &str, profile: &str) -> Option<Request> {
    match detail {
        Detail::Soul => Some(req(Method::Get, format!("/api/profiles/{}/soul", seg(id)), None)),
        Detail::Skill => Some(req(
            Method::Get,
            format!(
                "/api/skills/content?name={}{}",
                seg(id),
                profile_query(profile).replacen('?', "&", 1)
            ),
            None,
        )),
        Detail::Prompt => Some(req(Method::Get, format!("/api/cron/jobs/{}", seg(id)), None)),
        Detail::Rename => None,
    }
}

pub fn detail_text(detail: Detail, resp: &Value) -> String {
    match detail {
        Detail::Soul | Detail::Skill => s(resp, "content").to_owned(),
        Detail::Prompt => s(resp, "prompt").to_owned(),
        Detail::Rename => String::new(),
    }
}

pub fn save_detail_request(detail: Detail, id: &str, text: &str, profile: &str) -> Result<Request, String> {
    match detail {
        Detail::Soul => Ok(req(
            Method::Put,
            format!("/api/profiles/{}/soul", seg(id)),
            Some(json!({"content": text})),
        )),
        Detail::Skill => Ok(req(
            Method::Put,
            format!("/api/skills/content{}", profile_query(profile)),
            Some(json!({"name": id, "content": text})),
        )),
        Detail::Prompt => Ok(req(
            Method::Put,
            format!("/api/cron/jobs/{}", seg(id)),
            Some(json!({"updates": {"prompt": text}})),
        )),
        Detail::Rename => {
            let name = text.trim();
            if name.is_empty() {
                return Err("Enter the new profile name.".into());
            }
            Ok(req(
                Method::Patch,
                format!("/api/profiles/{}", seg(id)),
                Some(json!({"new_name": name})),
            ))
        }
    }
}

// ── Create form ─────────────────────────────────────────────────────────────────

/// The create form of a tab: three field labels (empty = hidden) and the button text.
/// `None` = the tab has no create form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Form {
    pub fields: [&'static str; 3],
    pub hints: [&'static str; 3],
    pub button: &'static str,
}

pub fn form(tab: Tab) -> Option<Form> {
    match tab {
        Tab::Profiles => Some(Form {
            fields: ["Profile name", "Copy settings from", "Description"],
            hints: [
                "lowercase, for example research",
                "optional profile name, for example default",
                "optional",
            ],
            button: "Create profile",
        }),
        Tab::Skills => Some(Form {
            fields: ["Skill name", "Description", "Category"],
            hints: [
                "lowercase-with-dashes",
                "one line: when should the agent use it?",
                "optional",
            ],
            button: "Create skill",
        }),
        Tab::Mcp => Some(Form {
            fields: ["Server name", "Command or URL", "Arguments"],
            hints: [
                "for example github",
                "npx, uvx, a path, or https://…",
                "space-separated, optional",
            ],
            button: "Add server",
        }),
        Tab::Cron => Some(Form {
            fields: ["Job name", "Schedule", "Prompt"],
            hints: [
                "optional",
                "every 30m · 0 9 * * * · 2026-12-01T09:00",
                "what the agent should do",
            ],
            button: "Schedule job",
        }),
        Tab::Tools | Tab::Memory => None,
    }
}

/// Build the create request from the form's three fields (already in display order).
pub fn create_request(tab: Tab, fields: [&str; 3], profile: &str) -> Result<Request, String> {
    let [a, b2, c] = fields.map(str::trim);
    let q = profile_query(profile);
    match tab {
        Tab::Profiles => {
            if a.is_empty() {
                return Err("Enter a profile name.".into());
            }
            let mut body = json!({"name": a});
            if !b2.is_empty() {
                body["clone_from"] = json!(b2);
            }
            if !c.is_empty() {
                body["description"] = json!(c);
            }
            Ok(req(Method::Post, "/api/profiles", Some(body)))
        }
        Tab::Skills => {
            if a.is_empty() || b2.is_empty() {
                return Err("A skill needs a name and a one-line description.".into());
            }
            let mut body = json!({"name": a, "content": skill_template(a, b2)});
            if !c.is_empty() {
                body["category"] = json!(c);
            }
            Ok(req(Method::Post, format!("/api/skills{q}"), Some(body)))
        }
        Tab::Mcp => {
            if a.is_empty() || b2.is_empty() {
                return Err("An MCP server needs a name and a command or URL.".into());
            }
            let body = if b2.starts_with("http://") || b2.starts_with("https://") {
                json!({"name": a, "url": b2})
            } else {
                // Split on whitespace only: no shell, no quoting rules, nothing expanded.
                let mut parts = b2.split_whitespace();
                let command = parts.next().unwrap_or_default();
                let args: Vec<&str> = parts.chain(c.split_whitespace()).collect();
                json!({"name": a, "command": command, "args": args})
            };
            Ok(req(Method::Post, format!("/api/mcp/servers{q}"), Some(body)))
        }
        Tab::Cron => {
            if b2.is_empty() || c.is_empty() {
                return Err("A scheduled job needs a schedule and a prompt.".into());
            }
            let mut body = json!({"schedule": b2, "prompt": c, "deliver": "local"});
            if !a.is_empty() {
                body["name"] = json!(a);
            }
            Ok(req(Method::Post, "/api/cron/jobs", Some(body)))
        }
        Tab::Tools | Tab::Memory => Err("This tab has nothing to create.".into()),
    }
}

/// A minimal valid SKILL.md (the backend requires frontmatter with name + description).
/// JSON string quoting is valid YAML, so any description text stays one scalar.
pub fn skill_template(name: &str, description: &str) -> String {
    let quoted = serde_json::to_string(description).unwrap_or_else(|_| "\"\"".into());
    format!("---\nname: {name}\ndescription: {quoted}\n---\n\n# {name}\n\n{description}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_round_trip() {
        for t in Tab::ALL {
            assert_eq!(Tab::from_index(t.index()), t);
        }
        assert_eq!(Tab::from_index(-1), Tab::Profiles);
        assert_eq!(Tab::from_index(99), Tab::Profiles);
    }

    #[test]
    fn list_paths_carry_profile_scope() {
        assert_eq!(list_requests(Tab::Skills, "")[0].path, "/api/skills");
        assert_eq!(list_requests(Tab::Skills, "swe")[0].path, "/api/skills?profile=swe");
        assert_eq!(list_requests(Tab::Memory, "ea")[1].path, "/api/curator?profile=ea");
        assert_eq!(list_requests(Tab::Cron, "swe")[0].path, "/api/cron/jobs?profile=all");
    }

    #[test]
    fn profiles_parse_and_protect_default() {
        let v = json!({"profiles": [
            {"name": "default", "is_default": true, "model": "m1", "skill_count": 3},
            {"name": "swe", "bot_title": "Software Engineer", "gateway_running": true, "description": "Codes\nmore"}
        ]});
        let items = parse(Tab::Profiles, &[v]);
        assert_eq!(items[0].title, "Default (default)");
        assert_eq!(items[0].subtitle, "m1 · 3 skills");
        assert!(!items[0].actions.iter().any(|a| a.id == "delete"));
        assert_eq!(items[1].title, "Software Engineer (swe)");
        assert_eq!(items[1].badge, "gateway on");
        assert!(items[1].subtitle.ends_with("Codes"));
        assert!(items[1].actions.iter().any(|a| a.id == "delete" && a.confirm));
        assert!(action_request(Tab::Profiles, "default", "delete", "").is_none());
    }

    #[test]
    fn skills_sorted_with_toggle() {
        let v = json!([
            {"name": "zeta", "enabled": false, "provenance": "agent"},
            {"name": "Alpha", "description": "Does A", "category": "dev"}
        ]);
        let items = parse(Tab::Skills, &[v]);
        assert_eq!(items[0].title, "Alpha");
        assert_eq!(items[0].toggle, Some(true));
        assert_eq!(items[0].subtitle, "dev · Does A");
        assert_eq!(items[1].toggle, Some(false));
        assert_eq!(items[1].badge, "agent");
        let r = toggle_request(Tab::Skills, "zeta", true, "swe").unwrap();
        assert_eq!(
            (r.method, r.path.as_str()),
            (Method::Put, "/api/skills/toggle?profile=swe")
        );
        assert_eq!(r.body.unwrap(), json!({"name": "zeta", "enabled": true}));
    }

    #[test]
    fn toolsets_and_mcp() {
        let t = parse(
            Tab::Tools,
            &[json!([{"name": "web", "label": "Web", "enabled": true, "configured": false, "tools": ["a", "b"]}])],
        );
        assert_eq!(t[0].subtitle, "2 tools");
        assert_eq!(t[0].badge, "needs setup");
        assert_eq!(
            toggle_request(Tab::Tools, "a b", false, "").unwrap().path,
            "/api/tools/toolsets/a%20b"
        );

        let m = parse(
            Tab::Mcp,
            &[json!({"servers": [
                {"name": "fs", "transport": "stdio", "command": "npx", "args": ["-y", "srv"], "enabled": false, "source": "config"},
                {"name": "p", "transport": "http", "url": "https://x", "source": "plugin", "plugin": "kit"}
            ]})],
        );
        assert_eq!(m[0].subtitle, "stdio · npx -y srv");
        assert_eq!(m[0].toggle, Some(false));
        assert!(m[0].actions.iter().any(|a| a.id == "remove"));
        assert_eq!(m[1].toggle, None, "plugin servers are read-only");
        assert!(!m[1].actions.iter().any(|a| a.id == "remove"));
    }

    #[test]
    fn memory_rows_and_actions() {
        let mem = json!({"active": "honcho", "builtin_files": {"memory": 2048, "user": 0},
            "providers": [{"name": "honcho", "available": true}, {"name": "mem0", "available": true}, {"name": "x", "available": false}]});
        let cur = json!({"enabled": true, "paused": true, "interval_hours": 24, "last_run_at": null});
        let items = parse(Tab::Memory, &[mem, cur]);
        assert_eq!(items[0].subtitle, "Agent notes · 2.0 KB");
        assert!(items[2].actions.iter().any(|a| a.id == "use"), "built-in can be chosen");
        let honcho = items.iter().find(|i| i.id == "provider:honcho").unwrap();
        assert_eq!(honcho.badge, "active");
        assert!(honcho.actions.is_empty());
        assert!(items.iter().find(|i| i.id == "provider:x").unwrap().actions.is_empty());
        let curator = items.iter().find(|i| i.id == CURATOR).unwrap();
        assert_eq!(curator.toggle, Some(false));
        assert!(curator.subtitle.contains("never run"));

        let r = action_request(Tab::Memory, USER_FILE, "reset", "").unwrap();
        assert_eq!(
            r.path, "/api/memory/reset?profile=default",
            "destructive routes name the profile"
        );
        assert_eq!(r.body.unwrap(), json!({"target": "user"}));
        assert_eq!(
            action_request(Tab::Memory, CURATOR, "run", "swe").unwrap().path,
            "/api/curator/run?profile=swe"
        );
        let r = action_request(Tab::Memory, BUILTIN_PROVIDER, "use", "").unwrap();
        assert_eq!(r.body.unwrap(), json!({"provider": ""}));
        let r = toggle_request(Tab::Memory, CURATOR, true, "").unwrap();
        assert_eq!(r.body.unwrap(), json!({"paused": false}));
        assert!(toggle_request(Tab::Memory, MEMORY_FILE, true, "").is_none());
    }

    #[test]
    fn cron_rows_and_mutations() {
        let v = json!([{"id": "abc", "name": "", "prompt": "Summarise news\nmore", "state": "paused",
            "enabled": false, "schedule_display": "0 9 * * *", "next_run_at": null, "deliver": "telegram"}]);
        let items = parse(Tab::Cron, &[v]);
        assert_eq!(items[0].title, "Summarise news");
        assert_eq!(items[0].toggle, Some(false));
        assert_eq!(items[0].subtitle, "0 9 * * * · to telegram");
        let r = toggle_request(Tab::Cron, "abc", true, "").unwrap();
        assert_eq!((r.method, r.path.as_str()), (Method::Post, "/api/cron/jobs/abc/resume"));
        assert!(items[0].actions.iter().any(|a| a.id == "run" && a.confirm));
        let r = save_detail_request(Detail::Prompt, "abc", "new", "").unwrap();
        assert_eq!(r.body.unwrap(), json!({"updates": {"prompt": "new"}}));
    }

    #[test]
    fn create_forms_validate_and_build_bodies() {
        assert!(create_request(Tab::Profiles, ["", "", ""], "").is_err());
        let r = create_request(Tab::Profiles, ["research", "default", ""], "").unwrap();
        assert_eq!(r.body.unwrap(), json!({"name": "research", "clone_from": "default"}));

        let r = create_request(Tab::Mcp, ["gh", "npx -y @x/server", "--flag v"], "swe").unwrap();
        assert_eq!(r.path, "/api/mcp/servers?profile=swe");
        assert_eq!(
            r.body.unwrap(),
            json!({"name": "gh", "command": "npx", "args": ["-y", "@x/server", "--flag", "v"]})
        );
        let r = create_request(Tab::Mcp, ["h", "https://mcp.example/sse", ""], "").unwrap();
        assert_eq!(r.body.unwrap(), json!({"name": "h", "url": "https://mcp.example/sse"}));

        assert!(create_request(Tab::Cron, ["n", "", "p"], "").is_err());
        let r = create_request(Tab::Cron, ["", "every 1h", "ping"], "").unwrap();
        assert_eq!(
            r.body.unwrap(),
            json!({"schedule": "every 1h", "prompt": "ping", "deliver": "local"})
        );
        assert!(create_request(Tab::Tools, ["a", "b", "c"], "").is_err());
        assert!(form(Tab::Tools).is_none());
    }

    #[test]
    fn skill_template_has_valid_frontmatter() {
        let t = skill_template("my-skill", "Use when: \"quotes\" and colons");
        assert!(t.starts_with("---\nname: my-skill\ndescription: \"Use when: \\\"quotes\\\" and colons\"\n---\n"));
    }

    #[test]
    fn detail_paths() {
        assert_eq!(
            detail_request(Detail::Skill, "a b", "swe").unwrap().path,
            "/api/skills/content?name=a%20b&profile=swe"
        );
        assert_eq!(
            detail_request(Detail::Skill, "x", "").unwrap().path,
            "/api/skills/content?name=x"
        );
        assert!(save_detail_request(Detail::Rename, "swe", "  ", "").is_err());
        let r = save_detail_request(Detail::Rename, "swe", "dev", "").unwrap();
        assert_eq!((r.method, r.body.unwrap()), (Method::Patch, json!({"new_name": "dev"})));
    }

    #[test]
    fn filter_matches_every_word() {
        let items = parse(
            Tab::Skills,
            &[json!([
                {"name": "github-pr", "description": "Open pull requests", "provenance": "bundled"},
                {"name": "notes", "description": "Obsidian vault", "provenance": "agent"}
            ])],
        );
        assert_eq!(filter(&items, "").len(), 2);
        assert_eq!(filter(&items, "PULL github")[0].id, "github-pr");
        assert_eq!(filter(&items, "agent")[0].id, "notes");
        assert!(filter(&items, "github obsidian").is_empty());
    }

    #[test]
    fn notices() {
        assert_eq!(
            action_notice(Tab::Mcp, "test", "fs", &json!({"ok": true, "tools": [{}, {}]})),
            "fs: connected, 2 tools"
        );
        assert_eq!(
            action_notice(Tab::Mcp, "test", "fs", &json!({"ok": false, "error": "boom"})),
            "fs: test failed — boom"
        );
        assert_eq!(
            action_notice(Tab::Memory, "reset", "MEMORY.md", &json!({"deleted": ["MEMORY.md"]})),
            "Deleted MEMORY.md."
        );
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1536), "1.5 KB");
    }
}
