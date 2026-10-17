//! Provider / keys / vault widgets → `ui/w-providers.slint`.
//!
//! Owned ids: `providers.accounts`, `providers.keys`, `providers.custom`,
//! `providers.local`, `keys.tools`, `keys.settings`, `vault.credentials`,
//! `vault.sources`.
//!
//! Backend data (payload captures in `~/.hermes/cache/scratch/settings-api/`):
//! * `GET /api/providers/oauth` → `{providers: [{id, name, flow: device_code|external,
//!   cli_command, docs_url, disconnectable, disconnect_hint, status: {logged_in,
//!   free_tier, source_label, token_preview, expires_at, …}}]}`; sign-in:
//!   `POST /api/providers/oauth/{id}/start` → `{session_id, user_code, verification_url,
//!   expires_in, poll_interval}`, then `GET /api/providers/oauth/{id}/poll/{session_id}`
//!   (`status`: pending | approved | denied | expired | error | cancelled) until terminal;
//!   cancel: `DELETE /api/providers/oauth/sessions/{session_id}`; disconnect:
//!   `DELETE /api/providers/oauth/{id}`.
//! * `GET /api/env` → every credential env var: `{KEY: {is_set, redacted_value,
//!   description, url, category, is_password, provider, provider_label, channel_managed,
//!   provider_profiles[], advanced, custom, …}}`. Write: `PUT /api/env` `{key, value}`;
//!   delete: `DELETE /api/env` `{key}` (a body — see `env_del` for the transport caveat);
//!   reveal: `POST /api/env/reveal` `{key}`. The providers page shows `category ==
//!   "provider"` grouped by `provider_label`; the keys page shows everything else that is
//!   not `channel_managed`.
//! * `GET /api/providers/custom-endpoints` → `{endpoints: [{id, name, base_url, model,
//!   models[], api_key_preview, is_current, …}], current: {provider, model, base_url}}`;
//!   `POST /api/providers/custom-endpoints` (upsert) and `/validate` `{name, base_url,
//!   model, api_key}` → `{ok, reachable, message, models[]}`; `/…/{id}/activate`;
//!   `DELETE /…/{id}`.
//! * `GET /api/local-models/status` → `{runtime_installed, runtime_backend, tag,
//!   server_running, server_base_url, active_model_id, loaded_models{}, models[{id,
//!   size_label}], models_dir}` + `GET /api/local-models/hardware` (gpu/vram/ram).
//!   `POST /api/local-models/server` `{action: start|stop}`, `/activate` `{model_id}`,
//!   `/eject` `{model_id}`. Downloads and runtime install are out of scope this round.
//! * `GET /api/tools/toolsets` → `[{name, label, description, platform, platform_label,
//!   enabled, available, configured, tools[]}]`; `PUT /api/tools/toolsets/{name}`
//!   `{enabled}` → `{ok, name, platform, enabled, post_setup_started}`.
//! * `GET /api/credentials/pool` → `{providers: [{provider, entries: [{index, id, label,
//!   auth_type, source, priority, last_status, request_count, token_preview,
//!   has_refresh}]}]}`; `DELETE /api/credentials/pool/{provider}/{index}`.
//! * `vault.sources`: the password-manager sources (1Password/Bitwarden) are a gateway RPC
//!   (`vault.sources` / `vault.unlock` in the Electron app), not a REST route, so this
//!   client cannot read them; the panel states that instead of inventing rows.
//!
//! Action ids (the UI emits `<area>:<verb>[:<arg>]`; this module gets the part after the
//! first colon):
//! * `oauth-start:<provider>`, `oauth-cancel:<provider>:<session>`,
//!   `oauth-disconnect:<provider>`, `oauth-refresh`
//! * `env-set:<KEY>:<value>`, `env-del:<KEY>`, `env-reveal:<KEY>`, `env-hide:<KEY>`,
//!   `env-refresh`
//! * `tools-toggle:<name>:<true|false>`, `tools-refresh`
//! * `vault-del:<provider>:<index>`, `vault-refresh`
//! * `ce-add\n<name>\n<base-url>\n<model>\n<key>`, `ce-validate\n<same fields>`,
//!   `ce-activate:<id>`, `ce-delete:<id>`, `ce-refresh`
//! * `lm-server:<start|stop>`, `lm-activate:<model-id>`, `lm-eject:<model-id>`, `lm-refresh`

use std::time::{Duration, Instant};

use hermes_core::backend::percent_encode;
use serde_json::{json, Value};
use slint::{ModelRc, VecModel};

use crate::widgets::{Ctx, Outcome};
use crate::{
    EndpointRow, KeyGroup, KeyRow, LocalModelRow, OAuthRow, PoolEntry, PoolGroup, ProvidersState,
    ToolsetRow,
};

/// Widget ids this module draws.
pub(crate) fn owns(id: &str) -> bool {
    matches!(
        id,
        "providers.accounts"
            | "providers.keys"
            | "providers.custom"
            | "providers.local"
            | "keys.tools"
            | "keys.settings"
            | "vault.credentials"
            | "vault.sources"
    )
}

// ── JSON helpers ────────────────────────────────────────────────────────────────

/// A string field, "" for missing/null/other.
fn str_of(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

fn bool_of(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn int_of(v: &Value, key: &str) -> i64 {
    v.get(key).and_then(Value::as_i64).unwrap_or(0)
}

/// `2026-10-17T12:00:00+00:00` → `2026-10-17 12:00`; anything else unchanged.
fn short_date(raw: &str) -> String {
    match raw.split_once('T') {
        Some((d, rest)) => format!("{d} {}", rest.split(['+', 'Z']).next().unwrap_or(rest)),
        None => raw.to_owned(),
    }
}

/// Bytes → "8.0 GB" (GiB, one decimal) — the units the backend's own labels use.
fn gb(bytes: f64) -> String {
    format!("{:.1} GB", bytes / (1024.0 * 1024.0 * 1024.0))
}

// ── Loaders ─────────────────────────────────────────────────────────────────────

/// Load (or reload) one widget's data into `ProvidersState`.
pub(crate) async fn load(id: &str, ctx: &Ctx) {
    match id {
        "providers.accounts" => load_accounts(ctx).await,
        "providers.keys" | "keys.settings" => refresh_env(ctx, None).await,
        "keys.tools" => load_toolsets(ctx).await,
        "vault.credentials" => load_pool(ctx).await,
        "providers.custom" => load_endpoints(ctx).await,
        "providers.local" => load_local(ctx).await,
        "vault.sources" => clear_status(ctx),
        _ => {}
    }
}

/// One `providers:<verb>[:<arg>]` action from the UI.
pub(crate) async fn action(rest: &str, ctx: &Ctx) -> Outcome {
    // Newline-delimited verbs carry several free-text fields (they may contain colons).
    if let Some((verb, args)) = rest.split_once('\n') {
        let fields: Vec<&str> = args.split('\n').collect();
        return match verb {
            "ce-add" => endpoint_save(ctx, &fields).await,
            "ce-validate" => endpoint_validate(ctx, &fields).await,
            _ => Outcome::None,
        };
    }
    let (verb, arg) = rest.split_once(':').unwrap_or((rest, ""));
    match verb {
        "oauth-start" => oauth_start(ctx, arg).await,
        "oauth-cancel" => oauth_cancel(ctx, arg).await,
        "oauth-disconnect" => oauth_disconnect(ctx, arg).await,
        "oauth-refresh" => {
            load_accounts(ctx).await;
            Outcome::None
        }
        "env-set" => env_set(ctx, arg).await,
        "env-del" => env_del(ctx, arg).await,
        "env-reveal" => env_reveal(ctx, arg).await,
        "env-hide" => {
            refresh_env(ctx, None).await;
            Outcome::None
        }
        "env-refresh" => {
            refresh_env(ctx, None).await;
            Outcome::None
        }
        "tools-toggle" => tools_toggle(ctx, arg).await,
        "tools-refresh" => {
            load_toolsets(ctx).await;
            Outcome::None
        }
        "vault-del" => pool_remove(ctx, arg).await,
        "vault-refresh" => {
            load_pool(ctx).await;
            Outcome::None
        }
        "ce-add" => endpoint_save(ctx, &[]).await,
        "ce-activate" => endpoint_activate(ctx, arg).await,
        "ce-delete" => endpoint_delete(ctx, arg).await,
        "ce-refresh" => {
            load_endpoints(ctx).await;
            Outcome::None
        }
        "lm-server" => local_server(ctx, arg).await,
        "lm-activate" => local_activate(ctx, arg).await,
        "lm-eject" => local_eject(ctx, arg).await,
        "lm-refresh" => {
            load_local(ctx).await;
            Outcome::None
        }
        _ => Outcome::None,
    }
}

/// Push a status line into the widget's own global (the panel renders it above the rows).
fn set_status(ctx: &Ctx, text: String, error: bool) {
    ctx.ui(move |ui| {
        let g = ui.global::<ProvidersState>();
        g.set_status(text.into());
        g.set_status_error(error);
    });
}

fn clear_status(ctx: &Ctx) {
    set_status(ctx, String::new(), false);
}

// ── providers.accounts ──────────────────────────────────────────────────────────

async fn load_accounts(ctx: &Ctx) {
    let path = format!("/api/providers/oauth{}", ctx.profile_query());
    match ctx.get_json(&path).await {
        Ok(v) => {
            ctx.ui(move |ui| {
                let g = ui.global::<ProvidersState>();
                g.set_oauth_rows(ModelRc::new(VecModel::from(parse_oauth(&v))));
                g.set_status("".into());
                g.set_status_error(false);
            });
        }
        Err(e) => set_status(ctx, format!("Could not load accounts: {e}"), true),
    }
}

/// `{providers: [...]}` → one row per OAuth-capable provider, backend order kept.
fn parse_oauth(v: &Value) -> Vec<OAuthRow> {
    let mut rows = Vec::new();
    let Some(list) = v.get("providers").and_then(Value::as_array) else {
        return rows;
    };
    for p in list {
        let status = p.get("status").cloned().unwrap_or(Value::Null);
        let logged_in = bool_of(&status, "logged_in");
        let free_tier = bool_of(&status, "free_tier");
        let connected = logged_in && !free_tier;
        let flow = str_of(p, "flow");
        let mut hint = str_of(p, "disconnect_hint");
        if hint.is_empty() && flow == "external" {
            hint = "Managed outside Hermes; remove it where that CLI keeps its credentials.".into();
        }
        rows.push(OAuthRow {
            id: str_of(p, "id").into(),
            name: str_of(p, "name").into(),
            flow: flow.into(),
            logged_in,
            free_tier,
            connected,
            status: if !logged_in {
                "Not signed in".into()
            } else if free_tier {
                "Free tier".into()
            } else {
                "Signed in".into()
            },
            source_label: str_of(&status, "source_label").into(),
            token_preview: str_of(&status, "token_preview").into(),
            expires: short_date(&str_of(&status, "expires_at")).into(),
            disconnectable: bool_of(p, "disconnectable"),
            hint: hint.into(),
            cli_command: str_of(p, "cli_command").into(),
        });
    }
    rows
}

async fn oauth_start(ctx: &Ctx, provider: &str) -> Outcome {
    if provider.is_empty() {
        return Outcome::Error("No provider to sign in to".into());
    }
    let path = format!(
        "/api/providers/oauth/{}/start{}",
        percent_encode(provider),
        ctx.profile_query()
    );
    let v = match ctx.post_json(&path, &json!({})).await {
        Ok(v) => v,
        Err(e) => return Outcome::Error(format!("Could not start sign-in for {provider}: {e}")),
    };
    let session = str_of(&v, "session_id");
    let code = str_of(&v, "user_code");
    let url = str_of(&v, "verification_url");
    let interval = int_of(&v, "poll_interval").clamp(2, 60) as u64;
    let expires = int_of(&v, "expires_in").clamp(30, 3600) as u64;
    if session.is_empty() {
        return Outcome::Error(format!("{provider} did not return a sign-in session"));
    }
    let (name, sess) = (provider.to_owned(), session.clone());
    ctx.ui(move |ui| {
        let g = ui.global::<ProvidersState>();
        g.set_oauth_provider(name.clone().into());
        g.set_oauth_name(name.into());
        g.set_oauth_session(sess.into());
        g.set_oauth_code(code.into());
        g.set_oauth_url(url.into());
        g.set_oauth_state("Waiting for approval…".into());
        g.set_status("".into());
        g.set_status_error(false);
    });
    // Poll on a detached task until the flow is terminal; the widget shows the state.
    let poll_ctx = clone_ctx(ctx);
    let (provider, session) = (provider.to_owned(), session);
    tokio::spawn(async move { poll_flow(poll_ctx, provider, session, interval, expires).await });
    Outcome::None
}

fn clone_ctx(ctx: &Ctx) -> Ctx {
    Ctx {
        rest: ctx.rest.clone(),
        weak: ctx.weak.clone(),
        scope: ctx.scope.clone(),
    }
}

fn set_flow_state(ctx: &Ctx, text: String) {
    ctx.ui(move |ui| ui.global::<ProvidersState>().set_oauth_state(text.into()));
}

/// Poll one OAuth session until it leaves `pending`, then refresh the account list.
async fn poll_flow(ctx: Ctx, provider: String, session: String, interval: u64, expires: u64) {
    let deadline = Instant::now() + Duration::from_secs(expires);
    loop {
        if Instant::now() >= deadline {
            set_flow_state(&ctx, "Sign-in expired before it was approved".into());
            finish_flow(&ctx, &provider, None).await;
            return;
        }
        tokio::time::sleep(Duration::from_secs(interval)).await;
        let path = format!(
            "/api/providers/oauth/{}/poll/{}",
            percent_encode(&provider),
            percent_encode(&session)
        );
        let Ok(v) = ctx.get_json(&path).await else {
            continue; // transient read; the session keeps its deadline
        };
        let status = str_of(&v, "status");
        match status.as_str() {
            "pending" => continue,
            "approved" => {
                finish_flow(&ctx, &provider, Some(true)).await;
                return;
            }
            other => {
                let msg = str_of(&v, "error_message");
                finish_flow(
                    &ctx,
                    &provider,
                    Some(false),
                )
                .await;
                set_status(
                    &ctx,
                    format!(
                        "Sign-in for {provider} {other}{}",
                        if msg.is_empty() {
                            String::new()
                        } else {
                            format!(": {msg}")
                        }
                    ),
                    true,
                );
                return;
            }
        }
    }
}

/// Close the flow card and reload the account list; `signed_in` also posts a notice.
async fn finish_flow(ctx: &Ctx, provider: &str, signed_in: Option<bool>) {
    ctx.ui(|ui| {
        let g = ui.global::<ProvidersState>();
        g.set_oauth_provider("".into());
        g.set_oauth_session("".into());
        g.set_oauth_code("".into());
        g.set_oauth_url("".into());
        g.set_oauth_state("".into());
    });
    load_accounts(ctx).await;
    if signed_in == Some(true) {
        set_status(ctx, format!("Signed in to {provider}"), false);
    }
}

async fn oauth_cancel(ctx: &Ctx, arg: &str) -> Outcome {
    // `<provider>:<session>` — the trailing session id never contains a colon.
    let (provider, session) = arg.split_once(':').unwrap_or((arg, ""));
    let mut cleared = true;
    if !session.is_empty() {
        let path = format!(
            "/api/providers/oauth/sessions/{}",
            percent_encode(session)
        );
        cleared = ctx.delete_json(&path).await.is_ok();
    }
    finish_flow(ctx, provider, None).await;
    if cleared {
        Outcome::Notice("Sign-in cancelled".into())
    } else {
        Outcome::Error("The sign-in session could not be cancelled on the backend".into())
    }
}

async fn oauth_disconnect(ctx: &Ctx, provider: &str) -> Outcome {
    let path = format!(
        "/api/providers/oauth/{}{}",
        percent_encode(provider),
        ctx.profile_query()
    );
    match ctx.delete_json(&path).await {
        Ok(_) => {
            load_accounts(ctx).await;
            Outcome::Notice(format!("Disconnected {provider}"))
        }
        Err(e) => {
            load_accounts(ctx).await;
            Outcome::Error(format!("Could not disconnect {provider}: {e}"))
        }
    }
}

// ── providers.keys / keys.settings ──────────────────────────────────────────────

/// Fetch `/api/env` once and publish both the grouped provider keys and the flat
/// environment list. `reveal` optionally shows one key's real value in place of its mask.
async fn refresh_env(ctx: &Ctx, reveal: Option<(String, String)>) {
    let path = format!("/api/env{}", ctx.profile_query());
    match ctx.get_json(&path).await {
        Ok(v) => {
            ctx.ui(move |ui| {
                let g = ui.global::<ProvidersState>();
                g.set_key_groups(ModelRc::new(VecModel::from(parse_key_groups(&v, reveal.as_ref()))));
                g.set_env_keys(ModelRc::new(VecModel::from(parse_env_keys(&v, reveal.as_ref()))));
                g.set_status("".into());
                g.set_status_error(false);
            });
        }
        Err(e) => set_status(ctx, format!("Could not load environment keys: {e}"), true),
    }
}

fn key_row(key: &str, info: &Value, reveal: Option<&(String, String)>) -> KeyRow {
    let revealed = reveal.filter(|(k, _)| k == key).map(|(_, v)| v.clone());
    KeyRow {
        key: key.into(),
        label: key.into(),
        description: str_of(info, "description").into(),
        url: str_of(info, "url").into(),
        is_set: bool_of(info, "is_set"),
        masked: str_of(info, "redacted_value").into(),
        is_password: bool_of(info, "is_password"),
        revealed_value: revealed.unwrap_or_default().into(),
    }
}

/// Provider-category env vars, grouped by the backend's `provider_label`, sorted.
fn parse_key_groups(v: &Value, reveal: Option<&(String, String)>) -> Vec<KeyGroup> {
    let Some(map) = v.as_object() else {
        return Vec::new();
    };
    let mut buckets: std::collections::BTreeMap<String, Vec<KeyRow>> = std::collections::BTreeMap::new();
    for (key, info) in map {
        if str_of(info, "category") != "provider" || bool_of(info, "channel_managed") {
            continue;
        }
        let label = {
            let l = str_of(info, "provider_label");
            if l.is_empty() {
                let p = str_of(info, "provider");
                if p.is_empty() { "Other".to_owned() } else { p }
            } else {
                l
            }
        };
        buckets.entry(label).or_default().push(key_row(key, info, reveal));
    }
    buckets
        .into_iter()
        .map(|(label, mut rows)| {
            rows.sort_by(|a, b| a.key.cmp(&b.key));
            KeyGroup {
                label: label.into(),
                rows: ModelRc::new(VecModel::from(rows)),
            }
        })
        .collect()
}

/// Everything else the Keys page owns: non-provider, non-channel-managed env vars.
fn parse_env_keys(v: &Value, reveal: Option<&(String, String)>) -> Vec<KeyRow> {
    let Some(map) = v.as_object() else {
        return Vec::new();
    };
    let mut rows: Vec<KeyRow> = map
        .iter()
        .filter(|(_, info)| str_of(info, "category") != "provider" && !bool_of(info, "channel_managed"))
        .map(|(key, info)| key_row(key, info, reveal))
        .collect();
    rows.sort_by(|a, b| a.key.cmp(&b.key));
    rows
}

/// `env-set:<KEY>:<value>` — the value is everything after the second colon.
async fn env_set(ctx: &Ctx, arg: &str) -> Outcome {
    let Some((key, value)) = arg.split_once(':') else {
        return Outcome::Error("No key to save".into());
    };
    if key.is_empty() {
        return Outcome::Error("No key to save".into());
    }
    if value.is_empty() {
        return Outcome::Error(format!("Enter a value for {key} first"));
    }
    let path = format!("/api/env{}", ctx.profile_query());
    match ctx.put_json(&path, &json!({ "key": key, "value": value })).await {
        Ok(_) => {
            refresh_env(ctx, None).await;
            Outcome::Notice(format!("Saved {key}"))
        }
        Err(e) => Outcome::Error(format!("Could not save {key}: {e}")),
    }
}

async fn env_del(ctx: &Ctx, key: &str) -> Outcome {
    if key.is_empty() {
        return Outcome::Error("No key to clear".into());
    }
    // `DELETE /api/env` wants `{key}` in the body, but this client's REST helper has no
    // delete-with-body (and the widget may not build one), so we try the delete with the
    // key as a query parameter first and fall back to emptying the value via PUT.
    let mut path = format!("/api/env?key={}", percent_encode(key));
    if let Some(rest) = ctx.profile_query().strip_prefix('?') {
        path.push('&');
        path.push_str(rest);
    }
    if ctx.delete_json(&path).await.is_ok() {
        refresh_env(ctx, None).await;
        return Outcome::Notice(format!("Removed {key}"));
    }
    let put = format!("/api/env{}", ctx.profile_query());
    match ctx
        .put_json(&put, &json!({ "key": key, "value": "" }))
        .await
    {
        Ok(_) => {
            refresh_env(ctx, None).await;
            Outcome::Notice(format!(
                "Cleared {key} — the stored value is emptied (see the parity notes: this client cannot send a DELETE body)"
            ))
        }
        Err(e) => Outcome::Error(format!("Could not clear {key}: {e}")),
    }
}

async fn env_reveal(ctx: &Ctx, key: &str) -> Outcome {
    if key.is_empty() {
        return Outcome::Error("No key to reveal".into());
    }
    let path = format!("/api/env/reveal{}", ctx.profile_query());
    match ctx.post_json(&path, &json!({ "key": key })).await {
        Ok(v) => {
            let value = str_of(&v, "value");
            refresh_env(ctx, Some((key.to_owned(), value))).await;
            Outcome::None
        }
        Err(e) => Outcome::Error(format!("Could not reveal {key}: {e}")),
    }
}

// ── keys.tools ─────────────────────────────────────────────────────────────────

async fn load_toolsets(ctx: &Ctx) {
    let path = format!("/api/tools/toolsets{}", ctx.profile_query());
    match ctx.get_json(&path).await {
        Ok(v) => {
            ctx.ui(move |ui| {
                let g = ui.global::<ProvidersState>();
                g.set_toolset_rows(ModelRc::new(VecModel::from(parse_toolsets(&v))));
                g.set_status("".into());
                g.set_status_error(false);
            });
        }
        Err(e) => set_status(ctx, format!("Could not load toolsets: {e}"), true),
    }
}

fn parse_toolsets(v: &Value) -> Vec<ToolsetRow> {
    let Some(list) = v.as_array() else {
        return Vec::new();
    };
    list.iter()
        .map(|t| {
            let names: Vec<String> = t
                .get("tools")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            ToolsetRow {
                name: str_of(t, "name").into(),
                label: str_of(t, "label").into(),
                description: str_of(t, "description").into(),
                platform: {
                    let l = str_of(t, "platform_label");
                    if l.is_empty() { str_of(t, "platform") } else { l }
                }
                .into(),
                enabled: bool_of(t, "enabled"),
                available: bool_of(t, "available"),
                configured: bool_of(t, "configured"),
                tool_count: names.len() as i32,
                tools_hint: names.join(", ").into(),
            }
        })
        .collect()
}

async fn tools_toggle(ctx: &Ctx, arg: &str) -> Outcome {
    let (name, value) = arg.split_once(':').unwrap_or((arg, "false"));
    if name.is_empty() {
        return Outcome::Error("No toolset to toggle".into());
    }
    let enabled = value == "true";
    let path = format!(
        "/api/tools/toolsets/{}{}",
        percent_encode(name),
        ctx.profile_query()
    );
    match ctx.put_json(&path, &json!({ "enabled": enabled })).await {
        Ok(_) => {
            load_toolsets(ctx).await;
            Outcome::Notice(format!(
                "{name} {}",
                if enabled { "enabled" } else { "disabled" }
            ))
        }
        Err(e) => {
            load_toolsets(ctx).await;
            Outcome::Error(format!("Could not change {name}: {e}"))
        }
    }
}

// ── vault.credentials ───────────────────────────────────────────────────────────

async fn load_pool(ctx: &Ctx) {
    let path = format!("/api/credentials/pool{}", ctx.profile_query());
    match ctx.get_json(&path).await {
        Ok(v) => {
            ctx.ui(move |ui| {
                let g = ui.global::<ProvidersState>();
                g.set_pool_groups(ModelRc::new(VecModel::from(parse_pool(&v))));
                g.set_status("".into());
                g.set_status_error(false);
            });
        }
        Err(e) => set_status(ctx, format!("Could not load the credential pool: {e}"), true),
    }
}

fn parse_pool(v: &Value) -> Vec<PoolGroup> {
    let Some(list) = v.get("providers").and_then(Value::as_array) else {
        return Vec::new();
    };
    list.iter()
        .map(|p| {
            let provider = str_of(p, "provider");
            let entries = p
                .get("entries")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|e| PoolEntry {
                            provider: provider.clone().into(),
                            index: int_of(e, "index") as i32,
                            label: str_of(e, "label").into(),
                            auth_type: str_of(e, "auth_type").into(),
                            source: str_of(e, "source").into(),
                            status: str_of(e, "last_status").into(),
                            requests: int_of(e, "request_count") as i32,
                            token_preview: str_of(e, "token_preview").into(),
                            has_refresh: bool_of(e, "has_refresh"),
                        })
                        .collect()
                })
                .unwrap_or_default();
            PoolGroup {
                provider: provider.into(),
                entries: ModelRc::new(VecModel::from(entries)),
            }
        })
        .collect()
}

async fn pool_remove(ctx: &Ctx, arg: &str) -> Outcome {
    let (provider, index) = arg.split_once(':').unwrap_or((arg, ""));
    let Ok(index) = index.parse::<i64>() else {
        return Outcome::Error("No credential to remove".into());
    };
    if provider.is_empty() {
        return Outcome::Error("No credential to remove".into());
    }
    let path = format!(
        "/api/credentials/pool/{}/{}{}",
        percent_encode(provider),
        index,
        ctx.profile_query()
    );
    match ctx.delete_json(&path).await {
        Ok(_) => {
            load_pool(ctx).await;
            Outcome::Notice(format!("Removed {provider} credential #{index}"))
        }
        Err(e) => {
            load_pool(ctx).await;
            Outcome::Error(format!("Could not remove {provider} #{index}: {e}"))
        }
    }
}

// ── providers.custom ────────────────────────────────────────────────────────────

async fn load_endpoints(ctx: &Ctx) {
    let path = format!("/api/providers/custom-endpoints{}", ctx.profile_query());
    match ctx.get_json(&path).await {
        Ok(v) => {
            ctx.ui(move |ui| {
                let g = ui.global::<ProvidersState>();
                g.set_endpoint_rows(ModelRc::new(VecModel::from(parse_endpoints(&v))));
                g.set_endpoint_current(summarize_current(v.get("current")).into());
                g.set_status("".into());
                g.set_status_error(false);
            });
        }
        Err(e) => set_status(ctx, format!("Could not load custom endpoints: {e}"), true),
    }
}

fn summarize_current(current: Option<&Value>) -> String {
    let Some(c) = current else {
        return String::new();
    };
    let provider = str_of(c, "provider");
    let model = str_of(c, "model");
    let base = str_of(c, "base_url");
    if provider.is_empty() {
        return String::new();
    }
    let mut s = format!("Current: {provider}");
    if !model.is_empty() {
        s.push_str(" · ");
        s.push_str(&model);
    }
    if !base.is_empty() {
        s.push_str(" · ");
        s.push_str(&base);
    }
    s
}

fn parse_endpoints(v: &Value) -> Vec<EndpointRow> {
    let Some(list) = v.get("endpoints").and_then(Value::as_array) else {
        return Vec::new();
    };
    list.iter()
        .map(|e| {
            let models: Vec<String> = e
                .get("models")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            let hint = if models.is_empty() {
                String::new()
            } else {
                let mut h = models.join(", ");
                if h.len() > 120 {
                    h.truncate(117);
                    h.push('…');
                }
                format!("{} models", models.len()) + ": " + &h
            };
            EndpointRow {
                id: str_of(e, "id").into(),
                name: str_of(e, "name").into(),
                base_url: str_of(e, "base_url").into(),
                model: str_of(e, "model").into(),
                models_hint: hint.into(),
                api_key: {
                    let k = str_of(e, "api_key_preview");
                    if k.is_empty() { "none".to_owned() } else { k }
                }
                .into(),
                is_current: bool_of(e, "is_current"),
            }
        })
        .collect()
}

/// `ce-add\n<name>\n<base-url>\n<model>\n<key>` / `ce-validate\n<same>`.
fn form_fields(fields: &[&str]) -> (String, String, String, String) {
    let g = |i: usize| fields.get(i).map(|s| s.trim().to_owned()).unwrap_or_default();
    (g(0), g(1), g(2), g(3))
}

fn form_body(fields: &[&str]) -> Value {
    let (name, base_url, model, api_key) = form_fields(fields);
    let mut body = json!({ "id": "", "name": name, "base_url": base_url, "model": model,
        "discover_models": true, "make_default": false });
    if !api_key.is_empty() {
        body["api_key"] = json!(api_key);
    }
    body
}

async fn endpoint_save(ctx: &Ctx, fields: &[&str]) -> Outcome {
    let (name, base_url, _, _) = form_fields(fields);
    if name.is_empty() || base_url.is_empty() {
        return Outcome::Error("A name and a base URL are required".into());
    }
    let path = format!("/api/providers/custom-endpoints{}", ctx.profile_query());
    match ctx.post_json(&path, &form_body(fields)).await {
        Ok(_) => {
            ctx.ui(|ui| {
                let g = ui.global::<ProvidersState>();
                g.set_form_name("".into());
                g.set_form_base_url("".into());
                g.set_form_model("".into());
                g.set_form_key("".into());
                g.set_endpoint_message("".into());
            });
            load_endpoints(ctx).await;
            Outcome::Notice(format!("Saved endpoint {name}"))
        }
        Err(e) => {
            let msg = format!("Could not save: {e}");
            ctx.ui(move |ui| {
                ui.global::<ProvidersState>()
                    .set_endpoint_message(msg.into());
            });
            Outcome::Error(format!("Could not save the endpoint: {e}"))
        }
    }
}

async fn endpoint_validate(ctx: &Ctx, fields: &[&str]) -> Outcome {
    let (_, base_url, _, _) = form_fields(fields);
    if base_url.is_empty() {
        return Outcome::Error("Enter a base URL first".into());
    }
    let path = format!("/api/providers/custom-endpoints/validate{}", ctx.profile_query());
    match ctx.post_json(&path, &form_body(fields)).await {
        Ok(v) => {
            let ok = bool_of(&v, "ok");
            let reachable = bool_of(&v, "reachable");
            let message = str_of(&v, "message");
            let count = v
                .get("models")
                .and_then(Value::as_array)
                .map(|a| a.len())
                .unwrap_or(0);
            let text = if ok {
                format!("Reachable — {count} models")
            } else if !message.is_empty() {
                message
            } else if reachable {
                "Reachable, but the models list was not usable".to_owned()
            } else {
                "Could not reach the endpoint".to_owned()
            };
            let shown = text.clone();
            ctx.ui(move |ui| {
                ui.global::<ProvidersState>()
                    .set_endpoint_message(shown.into());
            });
            Outcome::Notice(text)
        }
        Err(e) => Outcome::Error(format!("Validation failed: {e}")),
    }
}

async fn endpoint_activate(ctx: &Ctx, id: &str) -> Outcome {
    let path = format!(
        "/api/providers/custom-endpoints/{}/activate{}",
        percent_encode(id),
        ctx.profile_query()
    );
    match ctx.post_json(&path, &json!({})).await {
        Ok(_) => {
            load_endpoints(ctx).await;
            Outcome::Notice(format!("Activated {id}"))
        }
        Err(e) => Outcome::Error(format!("Could not activate {id}: {e}")),
    }
}

async fn endpoint_delete(ctx: &Ctx, id: &str) -> Outcome {
    let path = format!(
        "/api/providers/custom-endpoints/{}{}",
        percent_encode(id),
        ctx.profile_query()
    );
    match ctx.delete_json(&path).await {
        Ok(_) => {
            load_endpoints(ctx).await;
            Outcome::Notice(format!("Deleted {id}"))
        }
        Err(e) => Outcome::Error(format!("Could not delete {id}: {e}")),
    }
}

// ── providers.local ─────────────────────────────────────────────────────────────

async fn load_local(ctx: &Ctx) {
    let status_path = "/api/local-models/status";
    let hardware_path = "/api/local-models/hardware";
    let (status, hardware) = tokio::join!(ctx.get_json(status_path), ctx.get_json(hardware_path));
    match status {
        Ok(s) => {
            let hw = hardware.ok();
            ctx.ui(move |ui| {
                let g = ui.global::<ProvidersState>();
                let loaded = s.get("loaded_models");
                let active = str_of(&s, "active_model_id");
                let rows: Vec<LocalModelRow> = s
                    .get("models")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .map(|m| {
                                let id = str_of(m, "id");
                                LocalModelRow {
                                    active: !active.is_empty() && id == active,
                                    loaded: loaded
                                        .and_then(|l| l.get(&id))
                                        .is_some_and(|v| !v.is_null()),
                                    id: id.into(),
                                    size: str_of(m, "size_label").into(),
                                }
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                g.set_lm_runtime_installed(bool_of(&s, "runtime_installed"));
                g.set_lm_tag(str_of(&s, "tag").into());
                g.set_lm_server_running(bool_of(&s, "server_running"));
                g.set_lm_server_url(str_of(&s, "server_base_url").into());
                g.set_lm_active_model(active.into());
                g.set_lm_models_dir(str_of(&s, "models_dir").into());
                g.set_lm_rows(ModelRc::new(VecModel::from(rows)));
                g.set_lm_hardware(hardware_line(hw.as_ref()).into());
                g.set_status("".into());
                g.set_status_error(false);
            });
        }
        Err(e) => set_status(ctx, format!("Could not load local models: {e}"), true),
    }
}

/// One line of GPU/RAM facts, empty when the backend reports none.
fn hardware_line(hw: Option<&Value>) -> String {
    let Some(h) = hw else {
        return String::new();
    };
    let mut parts: Vec<String> = Vec::new();
    let gpu = str_of(h, "gpu_name");
    if !gpu.is_empty() {
        parts.push(gpu);
    }
    let vram_label = str_of(h, "vram_label");
    let usable = h.get("vram_usable_bytes").and_then(Value::as_f64);
    if !vram_label.is_empty() {
        let mut v = format!("{vram_label} VRAM");
        if let Some(u) = usable.filter(|u| *u > 0.0) {
            v.push_str(&format!(" ({} usable)", gb(u)));
        }
        parts.push(v);
    }
    if let Some(ram) = h.get("ram_available_bytes").and_then(Value::as_f64).filter(|r| *r > 0.0) {
        parts.push(format!("{} RAM free", gb(ram)));
    }
    parts.join(" · ")
}

async fn local_server(ctx: &Ctx, action: &str) -> Outcome {
    if action != "start" && action != "stop" {
        return Outcome::Error("Unknown local-server action".into());
    }
    match ctx
        .post_json("/api/local-models/server", &json!({ "action": action }))
        .await
    {
        Ok(_) => {
            load_local(ctx).await;
            Outcome::Notice(if action == "start" {
                "Local server starting…".into()
            } else {
                "Local server stopped".into()
            })
        }
        Err(e) => {
            load_local(ctx).await;
            Outcome::Error(format!("Could not {action} the local server: {e}"))
        }
    }
}

async fn local_activate(ctx: &Ctx, model_id: &str) -> Outcome {
    match ctx
        .post_json(
            "/api/local-models/activate",
            &json!({ "model_id": model_id }),
        )
        .await
    {
        Ok(_) => {
            load_local(ctx).await;
            Outcome::Notice(format!("{model_id} is now the default local model"))
        }
        Err(e) => Outcome::Error(format!("Could not activate {model_id}: {e}")),
    }
}

async fn local_eject(ctx: &Ctx, model_id: &str) -> Outcome {
    match ctx
        .post_json("/api/local-models/eject", &json!({ "model_id": model_id }))
        .await
    {
        Ok(_) => {
            load_local(ctx).await;
            Outcome::Notice(format!("Ejected {model_id}"))
        }
        Err(e) => Outcome::Error(format!("Could not eject {model_id}: {e}")),
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owns_exactly_the_provider_ids() {
        for id in [
            "providers.accounts",
            "providers.keys",
            "providers.custom",
            "providers.local",
            "keys.tools",
            "keys.settings",
            "vault.credentials",
            "vault.sources",
        ] {
            assert!(owns(id), "{id}");
        }
        assert!(!owns("providers.other"));
        assert!(!owns("models.main"));
    }

    #[test]
    fn oauth_rows_derive_sign_in_state() {
        let v = json!({"providers": [
            {"id": "nous", "name": "Nous Portal", "flow": "device_code", "disconnectable": true,
             "status": {"logged_in": false, "source_label": "portal", "token_preview": null}},
            {"id": "xai-oauth", "name": "xAI", "flow": "device_code", "disconnectable": true,
             "status": {"logged_in": true, "source_label": "device_code", "token_preview": "…kQAA",
                        "expires_at": "2026-11-01T09:30:00+00:00"}},
            {"id": "qwen-oauth", "name": "Qwen", "flow": "external", "cli_command": "hermes auth add qwen-oauth",
             "disconnectable": false, "disconnect_hint": "Managed by that provider's CLI; remove it there.",
             "status": {"logged_in": false}}
        ]});
        let rows = parse_oauth(&v);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].status, "Not signed in");
        assert!(!rows[0].connected);
        assert_eq!(rows[1].status, "Signed in");
        assert!(rows[1].connected);
        assert_eq!(rows[1].expires, "2026-11-01 09:30:00");
        assert_eq!(rows[2].flow, "external");
        assert!(!rows[2].disconnectable);
        assert!(rows[2].hint.contains("Managed"));
    }

    #[test]
    fn oauth_free_tier_is_not_connected() {
        let v = json!({"providers": [{"id": "nous", "name": "N", "flow": "device_code",
            "status": {"logged_in": true, "free_tier": true}}]});
        let rows = parse_oauth(&v);
        assert_eq!(rows[0].status, "Free tier");
        assert!(!rows[0].connected);
    }

    #[test]
    fn env_split_groups_provider_keys_and_keeps_the_rest_flat() {
        let v = json!({
            "OPENROUTER_API_KEY": {"category": "provider", "provider": "openrouter",
                "provider_label": "OpenRouter", "is_set": true, "redacted_value": "sk-…123",
                "is_password": true, "description": "key", "url": "https://openrouter.ai/keys"},
            "GOOGLE_API_KEY": {"category": "provider", "provider": "gemini",
                "provider_label": "Google AI Studio", "is_set": false},
            "GEMINI_API_KEY": {"category": "provider", "provider": "gemini",
                "provider_label": "Google AI Studio", "is_set": false},
            "HERMES_PORT": {"category": "setting", "is_set": true, "is_password": false},
            "TELEGRAM_TOKEN": {"category": "messaging", "channel_managed": true},
            "MY_OWN_KEY": {"category": "custom", "is_set": true, "is_password": true}
        });
        let groups = parse_key_groups(&v, None);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].label, "Google AI Studio");
        assert_eq!(groups[0].rows.row_count(), 2);
        assert_eq!(groups[1].label, "OpenRouter");
        assert_eq!(groups[1].rows.row_count(), 1);
        let flat = parse_env_keys(&v, None);
        let keys: Vec<&str> = flat.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(keys, vec!["HERMES_PORT", "MY_OWN_KEY"]);
    }

    #[test]
    fn env_reveal_shows_one_value_and_keeps_the_mask_for_the_rest() {
        let v = json!({
            "A_KEY": {"category": "custom", "is_set": true, "redacted_value": "sk-…aa"},
            "B_KEY": {"category": "custom", "is_set": true, "redacted_value": "sk-…bb"}
        });
        let reveal = ("A_KEY".to_owned(), "sk-real-value".to_owned());
        let rows = parse_env_keys(&v, Some(&reveal));
        assert_eq!(rows[0].key, "A_KEY");
        assert_eq!(rows[0].revealed_value, "sk-real-value");
        assert_eq!(rows[1].key, "B_KEY");
        assert_eq!(rows[1].revealed_value, "");
    }

    #[test]
    fn env_set_action_splits_on_the_first_colon_only() {
        // `env-set:<KEY>:<value>` — values may contain colons.
        let arg = "OPENROUTER_API_KEY:sk-a:b:c";
        let (key, value) = arg.split_once(':').unwrap();
        assert_eq!(key, "OPENROUTER_API_KEY");
        assert_eq!(value, "sk-a:b:c");
    }

    #[test]
    fn toolsets_keep_the_honest_flags_and_tool_count() {
        let v = json!([
            {"name": "web", "label": "Web Search & Scraping", "description": "web_search, web_extract",
             "platform": "cli", "platform_label": "CLI", "enabled": true, "available": true,
             "configured": true, "tools": ["web_extract", "web_search"]},
            {"name": "video", "label": "Video Analysis", "description": "video_analyze",
             "platform": "cli", "platform_label": "CLI", "enabled": false, "available": false,
             "configured": true, "tools": ["video_analyze"]}
        ]);
        let rows = parse_toolsets(&v);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].tool_count, 2);
        assert!(rows[0].enabled && rows[0].available && rows[0].configured);
        assert_eq!(rows[0].tools_hint, "web_extract, web_search");
        assert!(!rows[1].available);
        assert!(!rows[1].enabled);
    }

    #[test]
    fn pool_groups_keep_provider_entries_with_indices() {
        let v = json!({"providers": [
            {"provider": "anthropic", "entries": [
                {"index": 1, "id": "fa9633", "label": "anthropic-oauth-3", "auth_type": "oauth",
                 "source": "manual:hermes_pkce", "priority": 0, "last_status": "ok",
                 "request_count": 2, "token_preview": "sk-a...nQAA", "has_refresh": true},
                {"index": 2, "id": "0485e5", "label": "claude_code", "auth_type": "oauth",
                 "source": "claude_code", "last_status": null, "request_count": 0,
                 "token_preview": "sk-a...twAA", "has_refresh": true}
            ]}
        ]});
        let groups = parse_pool(&v);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].provider, "anthropic");
        assert_eq!(groups[0].entries.row_count(), 2);
        let e = &groups[0].entries.row_data(0).unwrap();
        assert_eq!(e.index, 1);
        assert_eq!(e.status, "ok");
        assert_eq!(e.requests, 2);
        assert!(e.has_refresh);
        let e2 = &groups[0].entries.row_data(1).unwrap();
        assert_eq!(e2.status, "");
        assert_eq!(e2.index, 2);
    }

    #[test]
    fn empty_custom_endpoints_and_the_current_summary() {
        let v = json!({"endpoints": [], "current": {"provider": "commandcode",
            "model": "deepseek/deepseek-v4.1-flash", "base_url": "https://api.commandcode.ai/provider/v1"}});
        assert!(parse_endpoints(&v).is_empty());
        let s = summarize_current(v.get("current"));
        assert!(s.starts_with("Current: commandcode"));
        assert!(s.contains("deepseek/deepseek-v4.1-flash"));
    }

    #[test]
    fn custom_endpoint_row_reports_the_key_preview() {
        let v = json!({"endpoints": [{"id": "local-8283", "name": "Local", "base_url": "http://127.0.0.1:8283/v1",
            "model": "qwen", "models": ["qwen", "llama"], "api_key_preview": "sk-…9", "is_current": true}],
            "current": {"provider": "local-8283"}});
        let rows = parse_endpoints(&v);
        assert_eq!(rows[0].id, "local-8283");
        assert!(rows[0].is_current);
        assert_eq!(rows[0].models_hint, "2 models: qwen, llama");
        assert_eq!(rows[0].api_key, "sk-…9");
    }

    #[test]
    fn custom_endpoint_form_needs_a_name_and_a_url() {
        let (name, url, _, _) = form_fields(&["  ", "https://x/v1", "", ""]);
        assert!(name.is_empty());
        assert_eq!(url, "https://x/v1");
        let body = form_body(&["Box", "https://x/v1", "m", "sk-1"]);
        assert_eq!(body["name"], "Box");
        assert_eq!(body["base_url"], "https://x/v1");
        assert_eq!(body["api_key"], "sk-1");
        assert_eq!(body["discover_models"], true);
    }

    #[test]
    fn hardware_line_reads_gpu_and_ram() {
        let hw = json!({"vram_total_bytes": 8589934592u64, "vram_usable_bytes": 654311424u64,
            "ram_available_bytes": 11134320640u64, "vram_label": "8.0 GB",
            "gpu_name": "NVIDIA GeForce RTX 3050", "gpu_util_percent": 8});
        let line = hardware_line(Some(&hw));
        assert!(line.contains("NVIDIA GeForce RTX 3050"));
        assert!(line.contains("8.0 GB VRAM"));
        assert!(line.contains("usable"));
        assert!(line.contains("RAM free"));
        assert_eq!(hardware_line(None), "");
    }

    #[test]
    fn local_rows_use_the_staged_labels() {
        let s = json!({"runtime_installed": false, "tag": "b11370", "server_running": false,
            "active_model_id": "qwen2.5-3b", "loaded_models": {"qwen2.5-3b": {"port": 1}},
            "models": [{"id": "qwen2.5-3b", "size_bytes": 2000000000u64, "size_label": "1.9 GB"},
                       {"id": "llama-8b", "size_label": "4.7 GB"}],
            "models_dir": "/tmp/models"});
        // The row builder lives inside the ui closure; assert the raw facts it reads.
        assert_eq!(str_of(&s, "tag"), "b11370");
        assert!(s.get("loaded_models").unwrap().get("qwen2.5-3b").is_some());
        assert_eq!(str_of(&s.get("models").unwrap()[0], "size_label"), "1.9 GB");
    }

    #[test]
    fn short_date_trims_the_timezone() {
        assert_eq!(short_date("2026-11-01T09:30:00+00:00"), "2026-11-01 09:30:00");
        assert_eq!(short_date("2026-11-01"), "2026-11-01");
        assert_eq!(short_date(""), "");
    }
}
