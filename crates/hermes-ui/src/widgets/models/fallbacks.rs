//! Controller bridge for local fallback drafts. Opaque routing fields never enter UI data.
use super::*;
use hermes_core::fallbacks::{self as core, Action, Draft};
use slint::Model;
use std::sync::atomic::{AtomicU64, Ordering};

static EPOCH: AtomicU64 = AtomicU64::new(1);

fn next_epoch() -> u64 {
    EPOCH.fetch_add(1, Ordering::Relaxed)
}

fn context(scope: &str, draft: &Draft) -> String {
    format!("{}:{}", hermes_core::backend::percent_encode(scope), draft.epoch())
}

struct DisplayRow {
    key: String,
    provider: String,
    label: String,
    model: String,
    options: Vec<String>,
}

fn push(ctx: &Ctx) {
    let (token, revision, dirty, options, rows) = {
        let guard = cache(&ctx.scope);
        let state = guard.as_ref().expect("cache");
        let Some(draft) = &state.fallbacks else { return };
        let rows = draft
            .rows
            .iter()
            .map(|row| {
                let provider = hermes_core::model_settings::provider(&state.options, row.provider());
                let label = provider
                    .and_then(|p| p["name"].as_str())
                    .unwrap_or(row.provider())
                    .to_owned();
                let options = provider
                    .and_then(|p| serde_json::from_value::<hermes_core::protocol::ModelOptionProvider>(p.clone()).ok())
                    .map(|p| p.model_ids())
                    .unwrap_or_default();
                DisplayRow {
                    key: row.key.clone(),
                    provider: row.provider().into(),
                    label,
                    model: row.model().into(),
                    options,
                }
            })
            .collect::<Vec<_>>();
        (
            context(&ctx.scope, draft),
            state.fb_revision,
            draft.needs_save(),
            state.options.clone(),
            rows,
        )
    };
    let scope = hermes_core::backend::percent_encode(&ctx.scope);
    let mut providers: Vec<(String, String)> = options["providers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            Some((
                p["slug"].as_str()?.to_owned(),
                p["name"].as_str().unwrap_or_default().to_owned(),
            ))
        })
        .collect();
    for row in &rows {
        if !row.provider.is_empty() && !providers.iter().any(|(p, _)| p == &row.provider) {
            providers.push((row.provider.clone(), row.label.clone()));
        }
    }
    ctx.ui(move |ui| {
        let state = ui.global::<ModelsState>();
        let changed_context = state.get_fb_context().as_str() != token;
        if changed_context {
            state.set_fb_context(token.into());
            state.set_fb_revision(revision);
            state.set_fb_save_failed(false);
            state.set_fb_saving(false);
        }
        if state.get_scope_token().as_str() != scope {
            state.set_picker_target(SharedString::new());
        }
        state.set_scope_token(scope.into());
        state.set_fb_ready(true);
        // The UI can be several commands ahead while a save awaits HTTP. Do not echo over it.
        if state.get_fb_revision() != revision {
            return;
        }
        state.set_fb_dirty(dirty);
        state.set_fb_providers(ModelRc::new(VecModel::from(
            providers.iter().map(|(v, l)| choice(v, l)).collect::<Vec<_>>(),
        )));
        let rows = rows
            .into_iter()
            .map(|row| FbRow {
                key: row.key.into(),
                provider: row.provider.into(),
                provider_label: row.label.into(),
                model: row.model.into(),
                options: ModelRc::new(VecModel::from(
                    row.options.iter().map(|id| choice(id, id)).collect::<Vec<_>>(),
                )),
            })
            .collect::<Vec<_>>();
        let existing = state.get_fb_rows();
        let same_keys = existing.row_count() == rows.len()
            && rows
                .iter()
                .enumerate()
                .all(|(i, row)| existing.row_data(i).is_some_and(|old| old.key == row.key));
        if same_keys && let Some(model) = existing.as_any().downcast_ref::<VecModel<FbRow>>() {
            // Keep the repeater and focused text input alive during ordinary typing.
            for (i, row) in rows.into_iter().enumerate() {
                model.set_row_data(i, row);
            }
        } else {
            state.set_fb_rows(ModelRc::new(VecModel::from(rows)));
        }
    });
}

pub(super) async fn load(ctx: &Ctx) {
    let scope = hermes_core::backend::percent_encode(&ctx.scope);
    ctx.ui(move |ui| {
        let state = ui.global::<ModelsState>();
        state.set_fb_ready(false);
        state.set_scope_token(scope.into());
        state.set_picker_target(SharedString::new());
    });
    // The original fallback editor uses the process-wide provider catalogue, not a
    // selected profile's currently applied model. Config reads/writes remain scoped.
    let (config, options) = tokio::join!(ctx.config(), ctx.get_json("/api/model/options?explicit_only=1"));
    let (config, options) = match (config, options) {
        (Ok(c), Ok(o)) => (c, o),
        _ => {
            return push_error(
                ctx,
                "Could not load fallback models. Retry without changing profiles.".into(),
            );
        }
    };
    {
        let mut guard = cache(&ctx.scope);
        let state = guard.as_mut().expect("cache");
        state.options = options;
        if let Some(draft) = &mut state.fallbacks {
            if draft.sync_external(next_epoch(), &config["fallback_providers"]) {
                state.fb_revision = 0;
            }
        } else {
            state.fallbacks = Some(Draft::new(next_epoch(), &config["fallback_providers"]));
            state.fb_revision = 0;
        }
    }
    push(ctx);
}

pub(super) async fn action(args: &str, ctx: &Ctx) -> Outcome {
    let request = match core::decode_action(args) {
        Ok(r) => r,
        Err(e) => return Outcome::Error(e),
    };
    {
        let mut guard = cache(&ctx.scope);
        let state = guard.as_mut().expect("cache");
        let Some(draft) = &mut state.fallbacks else {
            return Outcome::Error("Load the fallback page before editing.".into());
        };
        if request.context != context(&ctx.scope, draft) {
            return Outcome::Error("The fallback profile or rows changed; no change was saved.".into());
        }
        if request.verb == "flush" {
            if request.revision != state.fb_revision {
                return Outcome::None;
            }
        } else {
            let result = match request.verb.as_str() {
                "add" => {
                    draft.add();
                    Ok(())
                }
                "provider" => draft.set_provider(&request.row, &request.value).map(|_| ()),
                "model" => draft.set_model(&request.row, &request.value).map(|_| ()),
                "remove" => draft.remove(&request.row),
                "up" => draft.move_by(&request.row, -1).map(|_| ()),
                "down" => draft.move_by(&request.row, 1).map(|_| ()),
                _ => Err("Invalid fallback action; no change was saved.".into()),
            };
            if let Err(e) = result {
                return Outcome::Error(e);
            }
            state.fb_revision = request.revision;
        }
    }
    if request.verb == "flush" {
        save(ctx, &request).await
    } else {
        push(ctx);
        Outcome::None
    }
}

async fn save(ctx: &Ctx, request: &Action) -> Outcome {
    let expected = {
        let guard = cache(&ctx.scope);
        let draft = guard.as_ref().expect("cache").fallbacks.as_ref().expect("draft");
        if !draft.needs_save() {
            drop(guard);
            push(ctx);
            return Outcome::None;
        }
        draft.snapshot()
    };
    // Avoid overwriting an externally edited chain with an old local baseline.
    let current = match ctx.config().await {
        Ok(c) => c,
        Err(_) => return failed(ctx, "Could not read the saved fallback chain. Nothing was written."),
    };
    let external_change = {
        let mut guard = cache(&ctx.scope);
        let state = guard.as_mut().expect("cache");
        let changed = state
            .fallbacks
            .as_mut()
            .expect("draft")
            .sync_external(next_epoch(), &current["fallback_providers"]);
        if changed {
            state.fb_revision = 0;
        }
        changed
    };
    if external_change {
        push(ctx);
        return Outcome::Error("Fallback models changed outside this editor. The current chain was reloaded; your older draft was not written.".into());
    }
    ctx.ui(|ui| ui.global::<ModelsState>().set_fb_saving(true));
    let write = ctx
        .config_set("fallback_providers", Value::Array(expected.clone()))
        .await;
    // Read even after a transport failure: it may have happened after persistence.
    let verified = match ctx.config().await {
        Ok(actual) => cache(&ctx.scope)
            .as_mut()
            .expect("cache")
            .fallbacks
            .as_mut()
            .expect("draft")
            .acknowledge(&expected, &actual["fallback_providers"])
            .is_ok(),
        Err(_) => false,
    };
    let token = request.context.clone();
    let revision = request.revision;
    ctx.ui(move |ui| {
        let state = ui.global::<ModelsState>();
        if state.get_fb_context().as_str() != token {
            return;
        }
        state.set_fb_saving(false);
        state.set_fb_save_failed(!verified);
        if verified && state.get_fb_revision() == revision {
            state.set_fb_dirty(false);
            state.set_error(SharedString::new());
            state.set_notice("Fallback models saved".into());
        }
    });
    if verified {
        push(ctx);
        Outcome::Reload
    } else {
        let message = if write.is_ok() {
            "Fallback changes may have been saved, but could not be confirmed. Your draft is retained; retry to verify."
        } else {
            "Could not save fallback models. Your draft is retained. Check the connection, then retry."
        };
        Outcome::Error(message.into())
    }
}

fn failed(ctx: &Ctx, message: &str) -> Outcome {
    ctx.ui(|ui| {
        let state = ui.global::<ModelsState>();
        state.set_fb_saving(false);
        state.set_fb_save_failed(true);
    });
    Outcome::Error(message.into())
}
