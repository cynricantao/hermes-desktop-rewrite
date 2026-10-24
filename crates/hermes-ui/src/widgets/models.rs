//! Model widgets → `ui/w-models.slint`.
//!
//! Owned ids: `model.main` (default model picker), `model.auxiliary` (per-task models),
//! `model.moa` (Mixture of Agents), `fallback_providers` (the Fallback Models row whose
//! `Control::Fallback` kind renders as a list editor instead of a text field).
//!
//! Backend calls (all verified against the OpenAPI listing at
//! `~/.hermes/cache/scratch/hermes-openapi.json`, the Electron callers under
//! `apps/desktop/src/app/settings/` and the payload captures in
//! `~/.hermes/cache/scratch/settings-api/`):
//! * `GET /api/model/options?explicit_only=1` → `{provider, model, providers: [{slug, name,
//!   authenticated, models[], is_current, …}]}`; the top-level `provider`/`model` are the
//!   current default pair, so one fetch covers both the picker and the "current" mark.
//! * `POST /api/model/set` with `{scope: "main", provider, model}` → sets the default model.
//!   The same route with `{scope: "auxiliary", task, provider, model}` writes
//!   `auxiliary.<task>.*` (``provider: "auto", model: ""`` clears a task back to automatic).
//!   This is the endpoint the Electron app uses (`setModelAssignment` in
//!   `apps/desktop/src/api/models.ts`) and the one whose OpenAPI description names the
//!   `auxiliary.<task>.*` config keys, so it is used for both slots — a single write that
//!   also sets the endpoint a custom provider needs and reports stale auxiliary pins.
//! * `GET /api/model/auxiliary` → `{tasks: [...], main: {provider, model}}`.
//! * `GET|PUT /api/model/moa` — the full config is round-tripped; only the edited preset's
//!   fields change (the backend persists `presets` when the map is non-empty, so editing the
//!   flat fields would be a no-op).
//! * `fallback_providers` is plain config: read with `ctx.config()`, written with
//!   `ctx.config_set` (a deep-merged sparse patch, so hand-written keys on other entries
//!   survive).
//!
//! Action ids (the UI side emits them via `act(...)`):
//! * `models:main-scope:<encoded-profile>:stage-provider|stage-model:<value>` — edit a draft.
//! * `models:main-scope:<encoded-profile>:apply` — verify and save the staged pair.
//! * `models:main-scope:<encoded-profile>:default-effort|default-speed:<value>` — profile defaults.
//! * `models:pick:<provider>:<model>` — legacy picker bridge; stages without applying.
//! * `models:mfilter:<text>` / `models:pfilter:<text>` — filter the main list / the picker.
//! * `models:open:<target>` — open the picker on `main`, `aux.<task>`, `moa.ref`, `moa.agg`,
//!   `fb.<index>` or `fb.new`.
//! * `models:choose:<target>:<provider>:<model>` — pick from the open picker.
//! * `models:clear:<target>` — `aux.<task>` back to `automatic`.
//! * `models:fbup|fbdown|fbremove:<index>` — fallback list editing.
//! * `models:moa-enabled:<0|1>`, `models:moa-preset:<name>`, `models:moa-ref-enabled:<i>:<0|1>`,
//!   `models:moa-ref-remove:<i>`, `models:moa-fanout:<i>`, `models:moa-policy:<i>`.

use hermes_core::model_settings::{MainDefaults, ModelDraft};
use std::sync::{Mutex, MutexGuard};

use serde_json::{Value, json};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::widgets::{Ctx, Outcome};
use crate::{AuxRow, FbRow, MoaRef, ModelGroup, ModelsState};

/// Widget ids this module draws.
pub(crate) fn owns(id: &str) -> bool {
    matches!(
        id,
        "model.main" | "model.auxiliary" | "model.moa" | "fallback_providers"
    )
}

/// Built-in auxiliary tasks, in the order the backend serves them
/// (`i18n/en_aux_tasks.ts`); plugin-registered tasks arrive with their own `label`/`hint`.
const AUX_TASKS: &[(&str, &str, &str)] = &[
    ("vision", "Vision", "Image analysis"),
    ("compression", "Compression", "Context compaction"),
    ("skills_hub", "Skills hub", "Skill search"),
    ("approval", "Approval", "Smart auto-approve"),
    ("mcp", "MCP", "MCP tool routing"),
    ("title_generation", "Title gen", "Session titles"),
    ("review", "Review", "/review reviewer subagent"),
    ("voice_chat", "Voice chat", "Spoken voice-mode replies"),
    ("triage_specifier", "Triage specifier", "Kanban spec fleshing"),
    ("kanban_decomposer", "Kanban decomposer", "Task decomposition"),
    ("profile_describer", "Profile describer", "Auto profile descriptions"),
    ("curator", "Curator", "Skill-usage review"),
];

/// Fan-out cadences the panel offers (`every_n:<N>` values are kept as a third segment so an
/// untouched config still round-trips).
const FANOUTS: &[(&str, &str)] = &[("user_turn", "Per turn"), ("per_iteration", "Per iteration")];

// ── Module state ────────────────────────────────────────────────────────────────
//
// The panel is stateless between calls by contract (`load`/`action` are free functions), but
// a picker filter and the MoA editor both need the last payload the backend sent: the options
// catalog to re-filter without a round trip, and the MoA config to edit-and-PUT back whole.
// Both live here, keyed by the profile they came from, and are replaced on every load.

#[derive(Default)]
struct Cache {
    /// Profile the cached payloads belong to ("" = the default profile).
    scope: String,
    /// Last `/api/model/options` payload.
    options: Value,
    main_draft: Option<ModelDraft>,
    config: Option<Value>,
    /// Last `/api/model/moa` payload (the full object PUT back after an edit).
    moa: Value,
    /// Preset the MoA panel edits.
    preset: String,
    /// Main-list search text, so a reload keeps the filter.
    main_query: String,
    /// Picker search text.
    picker_query: String,
}

static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

/// The cache, reset when the open profile changes. Returns the guard over the `Option` so
/// callers use `as_ref()`/`as_mut()` and a missing entry is a loud `expect`, not a panic
/// on a null guard.
fn cache(scope: &str) -> MutexGuard<'static, Option<Cache>> {
    let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let stale = guard.as_ref().is_none_or(|c| c.scope != scope);
    if stale {
        *guard = Some(Cache {
            scope: scope.to_owned(),
            ..Cache::default()
        });
    }
    guard
}

// ── URLs and payload helpers ────────────────────────────────────────────────────

/// `path` with the open profile's `profile=` parameter appended (default profile: unchanged).
fn scoped(ctx: &Ctx, path: &str) -> String {
    let query = ctx.profile_query(); // "" or "?profile=<name>"
    if query.is_empty() {
        path.to_owned()
    } else if path.contains('?') {
        format!("{path}&{}", &query[1..])
    } else {
        format!("{path}{query}")
    }
}

/// The provider/model pairs of a `/api/model/options` payload, filtered by `query`
/// (matches a model id, or a provider name/slug which then keeps all of its models).
struct Groups {
    groups: Vec<GroupData>,
    providers: usize,
    matched: usize,
    total: usize,
}

/// A `ModelGroup` in plain (Send) form. `ModelGroup.models` is a Slint array, i.e. a
/// `ModelRc`, which cannot cross to the UI thread — so the model is built there by
/// [`group_model`] from this plain data.
#[derive(Clone)]
struct GroupData {
    slug: String,
    name: String,
    authenticated: bool,
    models: Vec<SharedString>,
}

/// Build the Slint model for a group list. Call **inside** a `ctx.ui` closure.
fn group_model(groups: Vec<GroupData>) -> ModelRc<ModelGroup> {
    let rows: Vec<ModelGroup> = groups
        .into_iter()
        .map(|g| ModelGroup {
            slug: g.slug.into(),
            name: g.name.into(),
            authenticated: g.authenticated,
            models: ModelRc::new(VecModel::from(g.models)),
        })
        .collect();
    ModelRc::new(VecModel::from(rows))
}

fn build_groups(options: &Value, query: &str) -> Groups {
    let needle = query.trim().to_lowercase();
    let mut out = Groups {
        groups: Vec::new(),
        providers: 0,
        matched: 0,
        total: 0,
    };
    let Some(providers) = options.get("providers").and_then(Value::as_array) else {
        return out;
    };
    for p in providers {
        let slug = p.get("slug").and_then(Value::as_str).unwrap_or_default();
        let name = p.get("name").and_then(Value::as_str).unwrap_or(slug);
        let authenticated = p.get("authenticated").and_then(Value::as_bool).unwrap_or(true);
        let all = p.get("models").and_then(Value::as_array).cloned().unwrap_or_default();
        out.total += all.len();
        let provider_hit =
            !needle.is_empty() && (name.to_lowercase().contains(&needle) || slug.to_lowercase().contains(&needle));
        let mut models: Vec<SharedString> = Vec::new();
        for id in all.iter().filter_map(Value::as_str) {
            if needle.is_empty() || provider_hit || id.to_lowercase().contains(&needle) {
                models.push(id.into());
            }
        }
        if models.is_empty() {
            continue;
        }
        out.providers += 1;
        out.matched += models.len();
        out.groups.push(GroupData {
            slug: slug.to_owned(),
            name: name.to_owned(),
            authenticated,
            models,
        });
    }
    out
}

fn count_label(groups: &Groups, query: &str, unit: &str) -> String {
    if query.trim().is_empty() {
        format!("{} providers · {} {unit}", groups.providers, groups.total)
    } else {
        format!("{} of {} {unit}", groups.matched, groups.total)
    }
}

fn text(value: &Value, key: &str) -> String {
    value.get(key).and_then(Value::as_str).unwrap_or_default().to_owned()
}

/// The preset names of a MoA payload, in payload order.
fn preset_names(moa: &Value) -> Vec<String> {
    moa.get("presets")
        .and_then(Value::as_object)
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default()
}

/// The preset `moa` uses unless the user picked another one.
fn default_preset_name(moa: &Value) -> String {
    let names = preset_names(moa);
    let named = text(moa, "default_preset");
    if !named.is_empty() && names.iter().any(|n| n == &named) {
        return named;
    }
    names.first().cloned().unwrap_or_else(|| "default".to_owned())
}

/// Apply `edit` to the preset `name` of a MoA payload (the payload itself for the legacy flat
/// shape, where `presets` is empty). Non-object presets are left alone.
fn with_preset(moa: &mut Value, name: &str, edit: impl FnOnce(&mut Value)) {
    let nested = moa
        .get("presets")
        .and_then(Value::as_object)
        .is_some_and(|o| !o.is_empty());
    let target = if nested {
        moa.get_mut("presets").and_then(|v| v.get_mut(name))
    } else {
        Some(moa)
    };
    if let Some(preset) = target.filter(|v| v.is_object()) {
        edit(preset);
    }
}

/// The fan-out values the panel offers for a stored `fanout` string.
fn fanout_values(current: &str) -> Vec<String> {
    let mut values: Vec<String> = FANOUTS.iter().map(|(v, _)| (*v).to_owned()).collect();
    if current.starts_with("every_n") {
        values.push(current.to_owned());
    }
    values
}

fn fanout_label(value: &str) -> String {
    FANOUTS
        .iter()
        .find(|(v, _)| *v == value)
        .map(|(_, label)| (*label).to_owned())
        .unwrap_or_else(|| match value.strip_prefix("every_n:") {
            Some(n) => format!("Every {n} turns"),
            None => value.to_owned(),
        })
}

// ── UI pushes ───────────────────────────────────────────────────────────────────

fn push_error(ctx: &Ctx, message: String) {
    ctx.ui(move |ui| ui.global::<ModelsState>().set_error(message.into()));
}

fn push_notice(ctx: &Ctx, message: &str) {
    let message = message.to_owned();
    ctx.ui(move |ui| {
        let state = ui.global::<ModelsState>();
        state.set_notice(message.into());
        state.set_error(SharedString::new());
    });
}

fn push_picker(ctx: &Ctx, target: &str, groups: Vec<GroupData>) {
    let target = target.to_owned();
    ctx.ui(move |ui| {
        let state = ui.global::<ModelsState>();
        state.set_picker_target(target.into());
        state.set_picker_search(SharedString::new());
        state.set_picker_groups(group_model(groups));
    });
}

/// Fill `ModelsState` from a MoA payload for the preset being edited.
fn push_moa(ctx: &Ctx, moa: &Value, preset: &str) {
    let names = preset_names(moa);
    let default = default_preset_name(moa);
    let selected = if names.iter().any(|n| n == preset) {
        preset.to_owned()
    } else {
        default.clone()
    };
    let presets = moa.get("presets").and_then(Value::as_object);
    let view = presets
        .filter(|o| !o.is_empty())
        .and_then(|o| o.get(&selected))
        .cloned()
        .unwrap_or_else(|| moa.clone());

    let enabled = view.get("enabled").and_then(Value::as_bool).unwrap_or(true);
    let refs: Vec<MoaRef> = view
        .get("reference_models")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|slot| MoaRef {
                    provider: text(slot, "provider").into(),
                    model: text(slot, "model").into(),
                    enabled: slot.get("enabled").and_then(Value::as_bool).unwrap_or(true),
                })
                .collect()
        })
        .unwrap_or_default();
    let aggregator = view.get("aggregator").cloned().unwrap_or(Value::Null);
    let fanout = text(&view, "fanout");
    let policy = text(&view, "degraded_reference_policy");
    let fanout_values = fanout_values(&fanout);
    let fanout_index = fanout_values.iter().position(|v| v == &fanout).unwrap_or(0) as i32;
    let fanout_labels: Vec<SharedString> = fanout_values.iter().map(|v| fanout_label(v).into()).collect();

    ctx.ui(move |ui| {
        let state = ui.global::<ModelsState>();
        state.set_moa_loaded(true);
        state.set_moa_enabled(enabled);
        state.set_moa_refs(ModelRc::new(VecModel::from(refs)));
        state.set_moa_aggregator_provider(text(&aggregator, "provider").into());
        state.set_moa_aggregator_model(text(&aggregator, "model").into());
        state.set_moa_fanout_options(ModelRc::new(VecModel::from(fanout_labels)));
        state.set_moa_fanout(fanout_index);
        state.set_moa_policy(if policy == "silent" { 1 } else { 0 });
        state.set_moa_presets(ModelRc::new(VecModel::from(
            names.into_iter().map(SharedString::from).collect::<Vec<_>>(),
        )));
        state.set_moa_preset(selected.into());
        state.set_moa_default_preset(default.into());
    });
}

fn choice(value: &str, label: &str) -> crate::SelectOption {
    crate::SelectOption {
        value: value.into(),
        label: label.into(),
        ..Default::default()
    }
}

fn effort_label(value: &str) -> String {
    match value {
        "none" => "Off".into(),
        "xhigh" => "X-High".into(),
        value => {
            let mut chars = value.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().to_string() + chars.as_str())
                .unwrap_or_default()
        }
    }
}

fn push_main(ctx: &Ctx) {
    let (options, draft, config, query) = {
        let guard = cache(&ctx.scope);
        let state = guard.as_ref().expect("cache");
        (
            state.options.clone(),
            state.main_draft.clone().unwrap_or_default(),
            state.config.clone(),
            state.main_query.clone(),
        )
    };
    let scope_token = hermes_core::backend::percent_encode(&ctx.scope);
    let applied = ModelDraft::applied(&options);
    let defaults = MainDefaults::read(&options, config.as_ref().unwrap_or(&Value::Null));
    let ready = config.is_some();
    let providers: Vec<_> = options["providers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let value = row["slug"].as_str()?;
            let label = row["name"].as_str().unwrap_or(value);
            Some((value.to_owned(), label.to_owned()))
        })
        .collect();
    let label = providers
        .iter()
        .find(|(value, _)| value == &draft.provider)
        .map(|(_, label)| label.clone())
        .unwrap_or_else(|| draft.provider.clone());
    let models = hermes_core::model_settings::provider(&options, &draft.provider)
        .and_then(|row| serde_json::from_value::<hermes_core::protocol::ModelOptionProvider>(row.clone()).ok())
        .map(|row| row.model_ids())
        .unwrap_or_default();
    let needle = query.to_lowercase();
    let models: Vec<_> = models
        .into_iter()
        .filter(|id| needle.is_empty() || id.to_lowercase().contains(&needle))
        .collect();
    ctx.ui(move |ui| {
        let state = ui.global::<ModelsState>();
        // A same-profile refresh must not overwrite UI edits whose commands are queued.
        if !state.get_main_draft_initialized() || state.get_main_draft_scope_token().as_str() != scope_token {
            state.set_main_last_provider(draft.provider.as_str().into());
            state.set_main_draft_provider(draft.provider.as_str().into());
            state.set_main_draft_model(draft.model.as_str().into());
            state.set_main_draft_scope_token(scope_token.as_str().into());
            state.set_main_draft_initialized(true);
        }
        state.set_main_provider(applied.provider.into());
        state.set_main_model(applied.model.into());
        let current_draft = state.get_main_draft_provider().as_str() == draft.provider
            && state.get_main_draft_model().as_str() == draft.model;
        state.set_main_provider_options(ModelRc::new(VecModel::from(
            providers
                .iter()
                .map(|(value, label)| choice(value, label))
                .collect::<Vec<_>>(),
        )));
        if current_draft {
            state.set_main_provider_label(label.into());
            state.set_main_model_options(ModelRc::new(VecModel::from(
                models.iter().map(|id| choice(id, id)).collect::<Vec<_>>(),
            )));
        }
        state.set_main_scope_token(scope_token.into());
        state.set_main_ready(true);
        state.set_main_defaults_ready(ready);
        state.set_main_reasoning_supported(defaults.reasoning);
        state.set_main_fast_supported(defaults.fast);
        state.set_main_ultrafast_supported(defaults.ultrafast);
        state.set_main_effort_options(ModelRc::new(VecModel::from(
            hermes_core::model_settings::REASONING_VALUES
                .iter()
                .map(|value| choice(value, &effort_label(value)))
                .collect::<Vec<_>>(),
        )));
        let mut speeds = vec![choice("normal", "Standard")];
        if defaults.fast {
            speeds.push(choice("fast", "Fast"));
        }
        if defaults.ultrafast {
            speeds.push(choice("ultrafast", "Ultrafast"));
        }
        state.set_main_speed_options(ModelRc::new(VecModel::from(speeds)));
        state.set_main_effort_label(effort_label(&defaults.effort).into());
        state.set_main_effort(defaults.effort.into());
        state.set_main_speed_label(
            match defaults.speed.as_str() {
                "fast" => "Fast",
                "ultrafast" => "Ultrafast",
                _ => "Standard",
            }
            .into(),
        );
        state.set_main_speed(defaults.speed.into());
        state.set_busy(false);
        state.set_error(SharedString::new());
    });
}

// ── Load ────────────────────────────────────────────────────────────────────────

/// Load (or reload) one widget's data into `ModelsState`.
pub(crate) async fn load(id: &str, ctx: &Ctx) {
    match id {
        "model.main" => load_main(ctx).await,
        "model.auxiliary" => load_aux(ctx).await,
        "model.moa" => load_moa(ctx).await,
        "fallback_providers" => load_fallbacks(ctx).await,
        _ => {}
    }
}

/// Fetch `/api/model/options` and keep it for the pickers/filters.
async fn fetch_options(ctx: &Ctx) -> Result<Value, String> {
    let payload = ctx.get_json(&scoped(ctx, "/api/model/options?explicit_only=1")).await?;
    cache(&ctx.scope).as_mut().expect("cache").options = payload.clone();
    Ok(payload)
}

async fn load_main(ctx: &Ctx) {
    let scope_token = hermes_core::backend::percent_encode(&ctx.scope);
    ctx.ui(move |ui| {
        let state = ui.global::<ModelsState>();
        state.set_busy(true);
        state.set_main_ready(false);
        state.set_main_scope_token(scope_token.into());
        state.set_main_defaults_ready(false);
    });
    let (options, config) = tokio::join!(fetch_options(ctx), ctx.config());
    let options = match options {
        Ok(value) => value,
        Err(error) => {
            ctx.ui(|ui| ui.global::<ModelsState>().set_busy(false));
            return push_error(ctx, format!("Could not load the model list: {error}"));
        }
    };
    let error = config.as_ref().err().cloned();
    let config = config.ok();
    {
        let mut guard = cache(&ctx.scope);
        let state = guard.as_mut().expect("cache");
        let applied = ModelDraft::applied(&options);
        // Only entering a profile initializes its draft. Reloads update Current separately.
        state.main_draft.get_or_insert(applied);
        state.config = config;
    }
    push_main(ctx);
    if let Some(error) = error {
        push_error(
            ctx,
            format!("Model loaded, but profile defaults are unavailable: {error}"),
        );
    }
}

async fn load_aux(ctx: &Ctx) {
    let payload = match ctx.get_json(&scoped(ctx, "/api/model/auxiliary")).await {
        Ok(v) => v,
        Err(e) => return push_error(ctx, format!("Could not load the auxiliary models: {e}")),
    };
    let rows: Vec<AuxRow> = payload
        .get("tasks")
        .and_then(Value::as_array)
        .map(|tasks| {
            tasks
                .iter()
                .map(|task| {
                    let key = text(task, "task");
                    let (label, hint) = match AUX_TASKS.iter().find(|(k, _, _)| *k == key) {
                        Some((_, label, hint)) => ((*label).to_owned(), (*hint).to_owned()),
                        None => {
                            let label = text(task, "label");
                            let hint = text(task, "hint");
                            (if label.is_empty() { key.clone() } else { label }, hint)
                        }
                    };
                    let provider = text(task, "provider");
                    let model = text(task, "model");
                    let inherit = text(task, "inherit_from");
                    let is_auto = provider.is_empty() || provider == "auto";
                    let route = if is_auto {
                        if inherit.is_empty() {
                            "Automatic · use the main model".to_owned()
                        } else {
                            format!("Follows {inherit}")
                        }
                    } else if model.is_empty() {
                        format!("{provider} · provider default")
                    } else {
                        format!("{provider} · {model}")
                    };
                    AuxRow {
                        task: key.into(),
                        label: label.into(),
                        hint: hint.into(),
                        provider: provider.into(),
                        model: model.into(),
                        route: route.into(),
                        is_auto,
                        inherit: inherit.into(),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    ctx.ui(move |ui| {
        let state = ui.global::<ModelsState>();
        state.set_aux_rows(ModelRc::new(VecModel::from(rows)));
        state.set_error(SharedString::new());
    });
}

async fn load_moa(ctx: &Ctx) {
    let payload = match ctx.get_json(&scoped(ctx, "/api/model/moa")).await {
        Ok(v) => v,
        Err(e) => return push_error(ctx, format!("Could not load the Mixture of Agents config: {e}")),
    };
    let preset = {
        let mut state = cache(&ctx.scope);
        let state = state.as_mut().expect("cache");
        state.moa = payload.clone();
        let names = preset_names(&payload);
        if !names.iter().any(|n| n == &state.preset) {
            state.preset = default_preset_name(&payload);
        }
        state.preset.clone()
    };
    push_moa(ctx, &payload, &preset);
}

async fn load_fallbacks(ctx: &Ctx) {
    let config = match ctx.config().await {
        Ok(c) => c,
        Err(e) => return push_error(ctx, format!("Could not load the fallback models: {e}")),
    };
    let rows: Vec<FbRow> = fallback_entries(&config)
        .iter()
        .map(|entry| FbRow {
            provider: text(entry, "provider").into(),
            model: text(entry, "model").into(),
        })
        .collect();
    ctx.ui(move |ui| {
        let state = ui.global::<ModelsState>();
        state.set_fb_rows(ModelRc::new(VecModel::from(rows)));
        state.set_error(SharedString::new());
    });
}

/// The `fallback_providers` list of a config, tolerating legacy `"provider/model"` strings.
fn fallback_entries(config: &Value) -> Vec<Value> {
    config
        .get("fallback_providers")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|entry| match entry {
                    Value::String(s) => match s.split_once('/') {
                        Some((provider, model)) if !provider.is_empty() => {
                            json!({"provider": provider, "model": model})
                        }
                        _ => json!({"provider": "", "model": s}),
                    },
                    other => other.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

// ── Writes ──────────────────────────────────────────────────────────────────────

/// `POST /api/model/set`, shared by the main slot and the auxiliary ones.
async fn assign(ctx: &Ctx, body: &Value, ok: String) -> Outcome {
    match ctx.post_json(&scoped(ctx, "/api/model/set"), body).await {
        Ok(result) => {
            if result.get("confirm_required").and_then(Value::as_bool) == Some(true)
                || result.get("ok").and_then(Value::as_bool) == Some(false)
            {
                let detail = text(&result, "confirm_message");
                let detail = if detail.is_empty() {
                    "the backend refused this model".to_owned()
                } else {
                    detail
                };
                return Outcome::Error(detail);
            }
            push_notice(ctx, &ok);
            Outcome::Reload
        }
        Err(e) => Outcome::Error(format!("Could not save the model: {e}")),
    }
}

async fn set_main(ctx: &Ctx, provider: &str, model: &str) -> Outcome {
    let result = stage_provider(ctx, provider);
    if matches!(result, Outcome::Error(_)) {
        return result;
    }
    let result = stage_model(ctx, model);
    push_main(ctx);
    result
}

fn push_draft_options(ctx: &Ctx) {
    let (options, draft, query) = {
        let guard = cache(&ctx.scope);
        let state = guard.as_ref().expect("cache");
        (
            state.options.clone(),
            state.main_draft.clone().unwrap_or_default(),
            state.main_query.clone(),
        )
    };
    let label = hermes_core::model_settings::provider(&options, &draft.provider)
        .and_then(|row| row["name"].as_str())
        .unwrap_or(&draft.provider)
        .to_owned();
    let models = hermes_core::model_settings::provider(&options, &draft.provider)
        .and_then(|row| serde_json::from_value::<hermes_core::protocol::ModelOptionProvider>(row.clone()).ok())
        .map(|row| row.model_ids())
        .unwrap_or_default();
    let needle = query.to_lowercase();
    let models: Vec<_> = models
        .into_iter()
        .filter(|id| needle.is_empty() || id.to_lowercase().contains(&needle))
        .collect();
    ctx.ui(move |ui| {
        let state = ui.global::<ModelsState>();
        if state.get_main_draft_provider().as_str() == draft.provider
            && state.get_main_draft_model().as_str() == draft.model
        {
            state.set_main_provider_label(label.into());
            state.set_main_model_options(ModelRc::new(VecModel::from(
                models.iter().map(|id| choice(id, id)).collect::<Vec<_>>(),
            )));
        }
    });
}

fn stage_provider(ctx: &Ctx, provider: &str) -> Outcome {
    {
        let mut guard = cache(&ctx.scope);
        let state = guard.as_mut().expect("cache");
        let Some(draft) = &mut state.main_draft else {
            return Outcome::Error("The model catalog has not loaded.".into());
        };
        if let Err(error) = draft.select_provider(&state.options, provider) {
            return Outcome::Error(error);
        }
        state.main_query.clear();
    }
    push_draft_options(ctx);
    Outcome::None
}

fn stage_model(ctx: &Ctx, model: &str) -> Outcome {
    {
        let mut guard = cache(&ctx.scope);
        let state = guard.as_mut().expect("cache");
        let Some(draft) = &mut state.main_draft else {
            return Outcome::Error("The model catalog has not loaded.".into());
        };
        draft.model = model.to_owned();
        state.main_query = model.to_owned();
    }
    push_draft_options(ctx);
    Outcome::None
}

async fn apply_main(ctx: &Ctx) -> Outcome {
    let (draft, body) = {
        let guard = cache(&ctx.scope);
        let state = guard.as_ref().expect("cache");
        let Some(draft) = state.main_draft.clone() else {
            return Outcome::Error("The model catalog has not loaded.".into());
        };
        let body = match draft.assignment(&state.options) {
            Ok(body) => body,
            Err(error) => return Outcome::Error(error),
        };
        (draft, body)
    };
    ctx.ui(|ui| ui.global::<ModelsState>().set_busy(true));
    let result = apply_main_verified(ctx, &draft, &body).await;
    ctx.ui(|ui| ui.global::<ModelsState>().set_busy(false));
    match result {
        Ok(()) => {
            push_notice(
                ctx,
                &format!("Default model set to {} · {}", draft.provider, draft.model),
            );
            Outcome::Reload
        }
        Err(error) => Outcome::Error(error),
    }
}

async fn apply_main_verified(ctx: &Ctx, draft: &ModelDraft, body: &Value) -> Result<(), String> {
    let result = ctx.post_json(&scoped(ctx, "/api/model/set"), body).await?;
    if result["confirm_required"].as_bool() == Some(true) || result["ok"].as_bool() == Some(false) {
        return Err(result["confirm_message"]
            .as_str()
            .unwrap_or("The backend refused this model.")
            .to_owned());
    }
    let saved = fetch_options(ctx).await?;
    if !draft.matches(&saved) {
        return Err("The saved model did not match this selection. Refresh before retrying.".into());
    }
    cache(&ctx.scope).as_mut().expect("cache").main_query.clear();
    Ok(())
}

async fn save_default(ctx: &Ctx, key: &str, value: &str) -> Outcome {
    let parsed = {
        let guard = cache(&ctx.scope);
        let state = guard.as_ref().expect("cache");
        let Some(config) = &state.config else {
            return Outcome::Error("Profile defaults have not loaded.".into());
        };
        match MainDefaults::read(&state.options, config).validate(key, value) {
            Ok(value) => value,
            Err(error) => {
                drop(guard);
                push_main(ctx);
                return Outcome::Error(error);
            }
        }
    };
    ctx.ui(|ui| ui.global::<ModelsState>().set_busy(true));
    let result = async {
        ctx.config_set(key, parsed).await?;
        let config = ctx.config().await?;
        let options = cache(&ctx.scope).as_ref().expect("cache").options.clone();
        let saved = MainDefaults::read(&options, &config);
        let actual = if key == "agent.reasoning_effort" {
            &saved.effort
        } else {
            &saved.speed
        };
        if actual != value {
            return Err("The backend did not save this model default.".to_owned());
        }
        cache(&ctx.scope).as_mut().expect("cache").config = Some(config);
        Ok(())
    }
    .await;
    // Do not depend on the controller's broader config refresh to release this panel.
    ctx.ui(|ui| ui.global::<ModelsState>().set_busy(false));
    match result {
        Ok(()) => {
            push_main(ctx);
            push_notice(
                ctx,
                if key == "agent.reasoning_effort" {
                    "Reasoning default saved"
                } else {
                    "Speed default saved"
                },
            );
            Outcome::Reload
        }
        Err(error) => {
            load_main(ctx).await;
            Outcome::Error(format!("Could not save this model default: {error}"))
        }
    }
}

async fn set_aux(ctx: &Ctx, task: &str, provider: &str, model: &str) -> Outcome {
    let body = json!({"scope": "auxiliary", "task": task, "provider": provider, "model": model});
    let label = AUX_TASKS
        .iter()
        .find(|(k, _, _)| *k == task)
        .map(|(_, label, _)| (*label).to_owned())
        .unwrap_or_else(|| task.to_owned());
    let ok = if provider == "auto" {
        format!("{label} back to automatic")
    } else {
        format!("{label} set to {provider} · {model}")
    };
    assign(ctx, &body, ok).await
}

/// Read the MoA payload, apply `edit` to the edited preset and PUT the whole object back.
async fn moa_edit(ctx: &Ctx, ok: &str, edit: impl FnOnce(&mut Value, &str)) -> Outcome {
    let (mut payload, preset) = {
        let state = cache(&ctx.scope);
        let state = state.as_ref().expect("cache");
        (state.moa.clone(), state.preset.clone())
    };
    if payload.is_null() {
        return Outcome::Error("The Mixture of Agents config is still loading.".to_owned());
    }
    edit(&mut payload, &preset);
    cache(&ctx.scope).as_mut().expect("cache").moa = payload.clone();
    match ctx.put_json(&scoped(ctx, "/api/model/moa"), &payload).await {
        Ok(_) => {
            push_notice(ctx, ok);
            Outcome::Reload
        }
        Err(e) => Outcome::Error(format!("Could not save the Mixture of Agents config: {e}")),
    }
}

async fn write_fallbacks(ctx: &Ctx, list: Vec<Value>, ok: &str) -> Outcome {
    match ctx.config_set("fallback_providers", Value::Array(list)).await {
        Ok(()) => {
            push_notice(ctx, ok);
            Outcome::Reload
        }
        Err(e) => Outcome::Error(format!("Could not save the fallback models: {e}")),
    }
}

// ── Actions ─────────────────────────────────────────────────────────────────────

/// One `models:<verb>[:<arg>]` action from the UI.
pub(crate) async fn action(rest: &str, ctx: &Ctx) -> Outcome {
    // Reject events still queued from the previously displayed profile.
    let rest = if let Some(scoped) = rest.strip_prefix("main-scope:") {
        let Some((token, action)) = scoped.split_once(':') else {
            return Outcome::Error("Invalid model action.".into());
        };
        if token != hermes_core::backend::percent_encode(&ctx.scope) {
            return Outcome::Error("The settings profile changed; no change was saved.".into());
        }
        action
    } else {
        rest
    };
    let (verb, args) = rest.split_once(':').unwrap_or((rest, ""));
    // Any action clears the previous confirmation line.
    ctx.ui(|ui| ui.global::<ModelsState>().set_notice(SharedString::new()));
    match verb {
        "stage-provider" => stage_provider(ctx, args),
        "stage-model" => stage_model(ctx, args),
        "apply" => apply_main(ctx).await,
        "reload-main" => {
            load_main(ctx).await;
            Outcome::None
        }
        "default-effort" => save_default(ctx, "agent.reasoning_effort", args).await,
        "default-speed" => save_default(ctx, "agent.service_tier", args).await,
        // Compatibility with an older picker: stage its selection, never bypass Apply.
        "pick" => {
            let (provider, model) = args.split_once(':').unwrap_or((args, ""));
            set_main(ctx, provider, model).await
        }
        "mfilter" => filter(ctx, "main", args).await,
        "pfilter" => filter(ctx, "picker", args).await,
        "open" => open_picker(ctx, args).await,
        "choose" => {
            let (target, rest) = args.split_once(':').unwrap_or((args, ""));
            let (provider, model) = rest.split_once(':').unwrap_or((rest, ""));
            choose(ctx, target, provider, model).await
        }
        "clear" => match args.strip_prefix("aux.") {
            Some(task) => set_aux(ctx, task, "auto", "").await,
            None => Outcome::Error(format!("Nothing to clear for “{args}”")),
        },
        "fbup" | "fbdown" | "fbremove" => fallback_edit(ctx, verb, args).await,
        "moa-enabled" => {
            let on = args == "1";
            moa_edit(
                ctx,
                if on {
                    "Mixture of Agents enabled"
                } else {
                    "Mixture of Agents disabled"
                },
                |moa, preset| {
                    with_preset(moa, preset, |p| p["enabled"] = json!(on));
                },
            )
            .await
        }
        "moa-preset" => select_preset(ctx, args),
        "moa-ref-enabled" => {
            let (index, on) = args.split_once(':').unwrap_or((args, "1"));
            let index: usize = parse_index(index);
            let on = on == "1";
            moa_edit(ctx, "Reference model updated", move |moa, preset| {
                with_preset(moa, preset, |p| {
                    if let Some(slot) = p
                        .get_mut("reference_models")
                        .and_then(Value::as_array_mut)
                        .and_then(|a| a.get_mut(index))
                    {
                        slot["enabled"] = json!(on);
                    }
                });
            })
            .await
        }
        "moa-ref-remove" => {
            let index = parse_index(args);
            let remaining = {
                let state = cache(&ctx.scope);
                let state = state.as_ref().expect("cache");
                preset_ref_count(&state.moa, &state.preset)
            };
            if remaining <= 1 {
                return Outcome::Error("A preset needs at least one reference model.".to_owned());
            }
            moa_edit(ctx, "Reference model removed", move |moa, preset| {
                with_preset(moa, preset, |p| {
                    if let Some(list) = p.get_mut("reference_models").and_then(Value::as_array_mut)
                        && index < list.len()
                    {
                        list.remove(index);
                    }
                });
            })
            .await
        }
        "moa-fanout" => {
            let index = parse_index(args);
            let value = {
                let state = cache(&ctx.scope);
                let state = state.as_ref().expect("cache");
                let current = preset_text(&state.moa, &state.preset, "fanout");
                fanout_values(&current).get(index).cloned()
            };
            match value {
                Some(value) => {
                    moa_edit(ctx, "Fan-out updated", move |moa, preset| {
                        with_preset(moa, preset, |p| p["fanout"] = json!(value));
                    })
                    .await
                }
                None => Outcome::Error("That fan-out option is gone; reopen the page.".to_owned()),
            }
        }
        "moa-policy" => {
            let index = parse_index(args);
            let value = if index == 0 { "loud" } else { "silent" };
            moa_edit(ctx, "Failed-advisor policy updated", move |moa, preset| {
                with_preset(moa, preset, |p| p["degraded_reference_policy"] = json!(value));
            })
            .await
        }
        other => Outcome::Error(format!("No handler for the “models:{other}” action")),
    }
}

fn parse_index(value: &str) -> usize {
    value.parse().unwrap_or(usize::MAX)
}

/// How many reference models the edited preset has.
fn preset_ref_count(moa: &Value, preset: &str) -> usize {
    let nested = moa
        .get("presets")
        .and_then(Value::as_object)
        .is_some_and(|o| !o.is_empty());
    let view = if nested {
        moa.get("presets").and_then(|p| p.get(preset))
    } else {
        Some(moa)
    };
    view.and_then(|p| p.get("reference_models"))
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default()
}

fn preset_text(moa: &Value, preset: &str, key: &str) -> String {
    let nested = moa
        .get("presets")
        .and_then(Value::as_object)
        .is_some_and(|o| !o.is_empty());
    let view = if nested {
        moa.get("presets").and_then(|p| p.get(preset))
    } else {
        Some(moa)
    };
    view.map(|p| text(p, key)).unwrap_or_default()
}

/// Re-render the MoA panel for another preset (no write).
fn select_preset(ctx: &Ctx, name: &str) -> Outcome {
    let payload = {
        let mut state = cache(&ctx.scope);
        let state = state.as_mut().expect("cache");
        if !preset_names(&state.moa).iter().any(|n| n == name) {
            return Outcome::Error(format!("No “{name}” preset."));
        }
        state.preset = name.to_owned();
        state.moa.clone()
    };
    push_moa(ctx, &payload, name);
    Outcome::None
}

/// Open the inline picker on a target, fetching the catalog when it is not cached yet.
async fn open_picker(ctx: &Ctx, target: &str) -> Outcome {
    let cached = {
        let state = cache(&ctx.scope);
        state.as_ref().expect("cache").options.clone()
    };
    let options = if cached.is_null() {
        match fetch_options(ctx).await {
            Ok(v) => v,
            Err(e) => return Outcome::Error(format!("Could not load the model list: {e}")),
        }
    } else {
        cached
    };
    let labels = build_groups(&options, "");
    cache(&ctx.scope).as_mut().expect("cache").picker_query.clear();
    push_picker(ctx, target, labels.groups);
    Outcome::None
}

/// Filter the main list or the open picker by `query`.
async fn filter(ctx: &Ctx, which: &str, query: &str) -> Outcome {
    let cached = {
        let state = cache(&ctx.scope);
        state.as_ref().expect("cache").options.clone()
    };
    let options = if cached.is_null() {
        match fetch_options(ctx).await {
            Ok(v) => v,
            Err(e) => return Outcome::Error(format!("Could not load the model list: {e}")),
        }
    } else {
        cached
    };
    let own = query.to_owned();
    let groups = build_groups(&options, query);
    if which == "main" {
        let count = count_label(&groups, query, "models");
        cache(&ctx.scope).as_mut().expect("cache").main_query = own;
        let rows = groups.groups;
        ctx.ui(move |ui| {
            let state = ui.global::<ModelsState>();
            state.set_main_groups(group_model(rows));
            state.set_main_count(count.into());
        });
    } else {
        cache(&ctx.scope).as_mut().expect("cache").picker_query = own.clone();
        let rows = groups.groups;
        ctx.ui(move |ui| {
            let state = ui.global::<ModelsState>();
            state.set_picker_search(own.into());
            state.set_picker_groups(group_model(rows));
        });
    }
    Outcome::None
}

/// One pick from the open picker, routed by target.
async fn choose(ctx: &Ctx, target: &str, provider: &str, model: &str) -> Outcome {
    let outcome = if target == "main" {
        set_main(ctx, provider, model).await
    } else if let Some(task) = target.strip_prefix("aux.") {
        set_aux(ctx, task, provider, model).await
    } else if target == "moa.ref" {
        let provider = provider.to_owned();
        let model = model.to_owned();
        moa_edit(ctx, "Reference model added", move |moa, preset| {
            with_preset(moa, preset, |p| {
                if !p.get("reference_models").is_some_and(Value::is_array) {
                    p["reference_models"] = json!([]);
                }
                if let Some(list) = p.get_mut("reference_models").and_then(Value::as_array_mut) {
                    list.push(json!({"provider": provider, "model": model, "enabled": true}));
                }
            });
        })
        .await
    } else if target == "moa.agg" {
        let slot = json!({"provider": provider, "model": model, "enabled": true});
        moa_edit(ctx, "Aggregator updated", move |moa, preset| {
            with_preset(moa, preset, |p| {
                let previous = p.get("aggregator").cloned().unwrap_or(Value::Null);
                let mut slot = slot;
                // Keep any reasoning_effort the user already had on the aggregator.
                if let Some(effort) = previous.get("reasoning_effort") {
                    slot["reasoning_effort"] = effort.clone();
                }
                p["aggregator"] = slot;
            });
        })
        .await
    } else if let Some(slot) = target.strip_prefix("fb.") {
        set_fallback(ctx, slot, provider, model).await
    } else {
        Outcome::Error(format!("Unknown model target “{target}”"))
    };
    if matches!(outcome, Outcome::Reload) {
        push_picker(ctx, "", Vec::new());
    }
    outcome
}

/// Replace (`fb.<index>`) or append (`fb.new`) one `fallback_providers` entry.
async fn set_fallback(ctx: &Ctx, slot: &str, provider: &str, model: &str) -> Outcome {
    let config = match ctx.config().await {
        Ok(c) => c,
        Err(e) => return Outcome::Error(format!("Could not read the fallback models: {e}")),
    };
    let mut list = fallback_entries(&config);
    if slot == "new" {
        list.push(json!({"provider": provider, "model": model}));
    } else {
        let index = parse_index(slot);
        if index >= list.len() {
            return Outcome::Error("That fallback row is gone; reopen the page.".to_owned());
        }
        // Keep hand-written keys (base_url, api_key, …) on the entry being re-pointed.
        if !list[index].is_object() {
            list[index] = json!({});
        }
        list[index]["provider"] = json!(provider);
        list[index]["model"] = json!(model);
    }
    let ok = if slot == "new" {
        format!("Fallback added: {provider} · {model}")
    } else {
        format!("Fallback set to {provider} · {model}")
    };
    write_fallbacks(ctx, list, &ok).await
}

/// Move or remove one `fallback_providers` entry.
async fn fallback_edit(ctx: &Ctx, verb: &str, slot: &str) -> Outcome {
    let config = match ctx.config().await {
        Ok(c) => c,
        Err(e) => return Outcome::Error(format!("Could not read the fallback models: {e}")),
    };
    let mut list = fallback_entries(&config);
    let index = parse_index(slot);
    if index >= list.len() {
        return Outcome::Error("That fallback row is gone; reopen the page.".to_owned());
    }
    let ok = match verb {
        "fbremove" => {
            list.remove(index);
            "Fallback removed".to_owned()
        }
        "fbup" if index > 0 => {
            list.swap(index - 1, index);
            "Fallback moved up".to_owned()
        }
        "fbdown" if index + 1 < list.len() => {
            list.swap(index, index + 1);
            "Fallback moved down".to_owned()
        }
        _ => return Outcome::None,
    };
    write_fallbacks(ctx, list, &ok).await
}
